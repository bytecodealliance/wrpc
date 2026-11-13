//! wRPC transport client handle

use core::future::poll_fn;
use core::pin::pin;
use core::task::{Poll, ready};

use anyhow::Context as _;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::{BytesCodec, Encoder as _};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::{Incoming, Outgoing};
use crate::mux::mux;
use crate::{Deferred as _, HeaderEncoder, HeaderRef, Paths as _, TupleDecode, TupleEncode};

/// Multiplexes the byte streams `tx` and `rx` of an invocation of function `func` on instance
/// `instance` declaring the async `paths`, buffering the invocation header, so that it is sent
/// along with the first frame
#[instrument(level = "trace", skip(tx, rx, paths))]
pub fn invoke(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    instance: &str,
    func: &str,
    paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
) -> std::io::Result<(Outgoing, Incoming)> {
    let mut buf = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance,
            name: func,
        },
        &mut buf,
    )?;
    Ok(mux(tx, rx, buf, paths))
}

/// Invoke function `func` on instance `instance` over the byte streams `tx` and `rx` using typed
/// `Params` and `Results`
///
/// Returns the synchronous results along with a future, which completes once the
/// asynchronous parameters are transmitted, the asynchronous results are received, if there
/// are any, and the invocation ends. Nothing is transmitted or received unless it is polled.
pub fn invoke_values<Params, Results>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    instance: &str,
    func: &str,
    params: Params,
) -> impl Future<
    Output = anyhow::Result<(
        Results,
        impl Future<Output = anyhow::Result<()>> + Send + 'static,
    )>,
> + Send
where
    Params: TupleEncode + Send,
    Results: TupleDecode + Send,
    <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
        core::error::Error + Send + Sync + 'static,
    <Results::Decoder as tokio_util::codec::Decoder>::Error:
        core::error::Error + Send + Sync + 'static,
{
    let io = invoke(tx, rx, instance, func, Results::Decoder::default().paths());
    async move {
        let mut buf = BytesMut::default();
        trace!("encoding parameters");
        let tx = params
            .encode(&mut Params::Encoder::default(), &mut buf)
            .context("failed to encode parameters")?;
        debug!("invoking function");
        let (mut outgoing, mut incoming) = io.context("failed to invoke function")?;
        let tx = async move {
            outgoing
                .encode(&mut BytesCodec::default(), buf)
                .await
                .context("failed to write sync parameters")?;
            let tx = tx.map(|tx| tx(&outgoing, &[])).transpose();
            let tx = tx.context("failed to index async parameters")?;
            outgoing
                .close()
                .await
                .context("failed to close parameter channel")?;
            if let Some(tx) = tx {
                debug!("transmitting async parameters");
                tx.await.context("failed to write async parameters")?;
            }
            anyhow::Ok(())
        }
        .in_current_span();
        let mut dec = Results::Decoder::default();
        debug!("receiving sync results");
        let mut tx = Some(Box::pin(tx));
        let results = {
            let mut results = pin!(incoming.decode(&mut dec));
            poll_fn(|cx| {
                if let Poll::Ready(results) = results.as_mut().poll(cx) {
                    let results = results.context("failed to receive sync results")?;
                    return Poll::Ready(results.context("incomplete results"));
                }
                if let Some(fut) = tx.as_mut() {
                    let res = ready!(fut.as_mut().poll(cx));
                    tx = None;
                    res?;
                }
                Poll::Pending
            })
            .await?
        };
        trace!("received sync results");
        let rx = dec.take_deferred();
        Ok((
            results,
            async move {
                try_join!(
                    async {
                        let closed = incoming.closed();
                        let rx = rx.map(|rx| rx(&incoming, &[])).transpose();
                        let rx = rx.context("failed to index async results")?;
                        try_join!(
                            async {
                                if let Some(rx) = rx {
                                    debug!("receiving async results");
                                    rx.await.context("failed to receive async results")?;
                                }
                                Ok(())
                            },
                            async {
                                incoming
                                    .read_end()
                                    .await
                                    .context("failed to receive sync results")
                            },
                        )?;
                        drop(incoming);
                        closed.await.context("failed to receive results")
                    },
                    async {
                        if let Some(tx) = tx {
                            tx.await?;
                        }
                        Ok(())
                    },
                )?;
                Ok(())
            }
            .in_current_span(),
        ))
    }
}

/// Invoke function `func` on instance `instance` over the byte streams `tx` and `rx` using typed
/// `Params` and `Results`
///
/// This is like [`invoke_values`], but it only resolves once all I/O is done
pub async fn invoke_values_blocking<Params, Results>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    instance: &str,
    func: &str,
    params: Params,
) -> anyhow::Result<Results>
where
    Params: TupleEncode + Send,
    Results: TupleDecode + Send,
    <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
        core::error::Error + Send + Sync + 'static,
    <Results::Decoder as tokio_util::codec::Decoder>::Error:
        core::error::Error + Send + Sync + 'static,
{
    let (ret, io) = invoke_values(tx, rx, instance, func, params).await?;
    trace!("awaiting I/O completion");
    io.await?;
    Ok(ret)
}

#[cfg(test)]
mod tests {
    use core::pin::Pin;

    use futures::Stream;

    use super::*;
    use crate::Connect;

    #[allow(clippy::manual_async_fn)]
    fn _invoke_values_send<T>() -> impl Future<
        Output = anyhow::Result<(
            Pin<Box<dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = String> + Send>>>> + Send>>,
        )>,
    > + Send
    where
        T: Connect<Context = ()> + Default,
    {
        async {
            let wrpc = T::default();
            let (tx, rx) = wrpc.connect(()).await?;
            let ((r0,), ..) =
                invoke_values(tx, rx, "wrpc-test:integration/async", "with-streams", ()).await?;
            Ok(r0)
        }
    }
}
