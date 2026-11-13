//! wRPC transport client handle

use core::future::poll_fn;
use core::pin::pin;
use core::task::{Poll, ready};

use anyhow::Context as _;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::{Decoder, Encoder};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::{Incoming, Outgoing};
use crate::mux::{egress, ingress};
use crate::{Deferred, HeaderEncoder, HeaderRef, Paths};

/// Invokes function `func` on instance `instance` over the byte streams `tx` and `rx`, encoding
/// `args` using `enc` and decoding the results using `dec`
///
/// `args` are encoded before this returns, the invocation header is sent along with the first
/// frame. Returns the synchronous results along with a future, which completes once the
/// asynchronous parameters are transmitted, the asynchronous results are received, if there
/// are any, and the invocation ends. Nothing is transmitted or received unless it is polled.
#[instrument(level = "trace", skip(tx, rx, enc, args, dec))]
pub fn invoke<E, T, D>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    instance: &str,
    func: &str,
    enc: E,
    args: T,
    mut dec: D,
) -> impl Future<
    Output = anyhow::Result<(
        D::Item,
        impl Future<Output = anyhow::Result<()>> + Send + 'static,
    )>,
> + Send
where
    E: Encoder<T> + Deferred<Outgoing>,
    E::Error: core::error::Error + From<std::io::Error> + Send + Sync + 'static,
    D: Decoder + Deferred<Incoming> + Paths + Send,
    D::Item: Send,
    D::Error: core::error::Error + Send + Sync + 'static,
{
    let mut buf = BytesMut::default();
    let header = HeaderEncoder.encode(
        HeaderRef {
            instance,
            name: func,
        },
        &mut buf,
    );
    let tx = header.map(|()| egress(tx, buf).send(enc, args));
    let mut incoming = ingress(rx, dec.paths());
    async move {
        let tx = tx.context("failed to encode invocation header")?;
        let tx = tx.context("failed to encode parameters")?;
        debug!("invoking function");
        let tx = async move {
            debug!("transmitting parameters");
            tx.await.context("failed to transmit parameters")
        }
        .in_current_span();
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
    .in_current_span()
}

/// Invokes function `func` on instance `instance`, see [`invoke`]
///
/// This is like [`invoke`], but it only resolves once all I/O is done
pub async fn invoke_blocking<E, T, D>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    instance: &str,
    func: &str,
    enc: E,
    args: T,
    dec: D,
) -> anyhow::Result<D::Item>
where
    E: Encoder<T> + Deferred<Outgoing>,
    E::Error: core::error::Error + From<std::io::Error> + Send + Sync + 'static,
    D: Decoder + Deferred<Incoming> + Paths + Send,
    D::Item: Send,
    D::Error: core::error::Error + Send + Sync + 'static,
{
    let (ret, io) = invoke(tx, rx, instance, func, enc, args, dec).await?;
    trace!("awaiting I/O completion");
    io.await?;
    Ok(ret)
}

#[cfg(test)]
mod tests {
    use core::pin::Pin;

    use futures::Stream;

    use super::*;
    use crate::{Connect, Decode, Encode};

    #[allow(clippy::manual_async_fn)]
    fn _invoke_send<T>() -> impl Future<
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
            let ((r0,), ..) = invoke(
                tx,
                rx,
                "wrpc-test:integration/async",
                "with-streams",
                <() as Encode>::Encoder::default(),
                (),
                <(
                    Pin<
                        Box<
                            dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = String> + Send>>>>
                                + Send,
                        >,
                    >,
                ) as Decode>::Decoder::default(),
            )
            .await?;
            Ok((r0,))
        }
    }
}
