#![allow(clippy::type_complexity)]

#[cfg(feature = "frame")]
pub mod frame;

mod value;

#[cfg(feature = "frame")]
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, FrameRef};
pub use value::*;

use core::future::Future;
use core::pin::{pin, Pin};

use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::{SinkExt as _, Stream, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::{select, try_join};
use tokio_util::codec::{Encoder as _, FramedRead, FramedWrite};
use tracing::{debug, instrument, trace, Instrument as _, Span};

/// `Index` implementations are capable of multiplexing underlying connections using a particular
/// structural `path`
pub trait Index<T> {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> anyhow::Result<T>;
}

/// Client-side handle to a wRPC transport
pub trait Invoke: Send + Sync + 'static {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync + Unpin + 'static;

    /// Incoming multiplexed byte stream
    type Incoming: AsyncRead + Index<Self::Incoming> + Send + Sync + Unpin + 'static;

    /// Invoke function `func` on instance `instance`
    fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: &[impl AsRef<[Option<usize>]> + Send + Sync],
    ) -> impl Future<Output = anyhow::Result<(Self::Outgoing, Self::Incoming)>> + Send;

    /// Invoke function `func` on instance `instance` using typed `Params` and `Results`
    #[instrument(level = "trace", skip(self, cx, params, paths))]
    fn invoke_values<Params, Results>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: &[impl AsRef<[Option<usize>]> + Send + Sync],
    ) -> impl Future<
        Output = anyhow::Result<(
            Results,
            Option<impl Future<Output = anyhow::Result<()>> + Send + 'static>,
        )>,
    > + Send
    where
        Params: TupleEncode<Self::Outgoing> + Send,
        Results: TupleDecode<Self::Incoming> + Send,
        <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
            std::error::Error + Send + Sync + 'static,
        <Results::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let mut buf = BytesMut::default();
            let mut enc = Params::Encoder::default();
            trace!("encoding parameters");
            enc.encode(params, &mut buf)
                .context("failed to encode parameters")?;
            debug!("invoking function");
            let (mut outgoing, incoming) = self
                .invoke(cx, instance, func, buf.freeze(), paths)
                .await
                .context("failed to invoke function")?;
            outgoing
                .shutdown()
                .await
                .context("failed to shutdown synchronous parameter channel")?;
            let mut tx = enc.take_deferred().map(|tx| {
                tokio::spawn(
                    async {
                        debug!("transmitting async parameters");
                        tx(outgoing.into(), Vec::with_capacity(8))
                            .await
                            .context("failed to write async parameters")
                    }
                    .in_current_span(),
                )
            });

            let mut dec = FramedRead::new(incoming, Results::Decoder::default());
            let results = async {
                debug!("receiving sync results");
                dec.try_next()
                    .await
                    .context("failed to receive sync results")?
                    .context("incomplete results")
            };
            let results = if let Some(mut fut) = tx.take() {
                let mut results = pin!(results);
                select! {
                    res = &mut results => {
                        tx = Some(fut);
                        res?
                    }
                    res = &mut fut => {
                        res??;
                        results.await?
                    }
                }
            } else {
                results.await?
            };
            trace!("received sync results");
            let rx = dec.decoder_mut().take_deferred();
            Ok((
                results,
                (tx.is_some() || rx.is_some()).then_some(async {
                    match (tx, rx) {
                        (Some(tx), Some(rx)) => {
                            try_join!(
                                async {
                                    debug!("receiving async results");
                                    rx(dec.into_inner().into(), Vec::with_capacity(8))
                                        .await
                                        .context("receiving async results failed")
                                },
                                async { tx.await.context("transmitting async parameters failed")? }
                            )?;
                        }
                        (Some(tx), None) => {
                            tx.await.context("transmitting async parameters failed")??;
                        }
                        (None, Some(rx)) => {
                            debug!("receiving async results");
                            rx(dec.into_inner().into(), Vec::with_capacity(8))
                                .await
                                .context("receiving async results failed")?;
                        }
                        _ => {}
                    }
                    Ok(())
                }),
            ))
        }
    }

    /// Invoke function `func` on instance `instance` using typed `Params` and `Results`
    /// This is like [`Self::invoke_values`], but it only results once all I/O is done
    #[instrument(level = "trace", skip_all)]
    fn invoke_values_blocking<Params, Results>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: &[impl AsRef<[Option<usize>]> + Send + Sync],
    ) -> impl Future<Output = anyhow::Result<Results>> + Send
    where
        Params: TupleEncode<Self::Outgoing> + Send,
        Results: TupleDecode<Self::Incoming> + Send,
        <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
            std::error::Error + Send + Sync + 'static,
        <Results::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let (ret, io) = self
                .invoke_values(cx, instance, func, params, paths)
                .await?;
            if let Some(io) = io {
                io.await?;
            }
            Ok(ret)
        }
    }
}

/// Server-side handle to a wRPC transport
pub trait Serve: Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync + Unpin + 'static;

    /// Incoming multiplexed byte stream
    type Incoming: AsyncRead + Index<Self::Incoming> + Send + Sync + Unpin + 'static;

    /// Serve function `func` from instance `instance`
    fn serve<P: AsRef<[Option<usize>]> + Send + Sync + 'static>(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[P]>> + Send + Sync + 'static,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
                + Send
                + 'static,
        >,
    > + Send;

    /// Serve function `func` from instance `instance` using typed `Params` and `Results`
    #[instrument(level = "trace", skip(self, paths))]
    fn serve_values<P, Params, Results>(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[P]>> + Send + Sync + 'static,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<(
                        Self::Context,
                        Params,
                        Option<
                            impl Future<Output = std::io::Result<()>> + Sync + Send + Unpin + 'static,
                        >,
                        impl FnOnce(
                            Results,
                        )
                            -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Sync + Send>>,
                    )>,
                > + Send
                + 'static,
        >,
    > + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync + 'static,
        Params: TupleDecode<Self::Incoming> + Send + Sync + 'static,
        Results: TupleEncode<Self::Outgoing> + Send + Sync + 'static,
        <Params::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
        <Results::Encoder as tokio_util::codec::Encoder<Results>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let invocations = self.serve(instance, func, paths).await?;
            let span = Span::current();
            Ok(invocations.and_then(move |(cx, outgoing, incoming)| {
                async {
                    let mut dec = FramedRead::new(incoming, Params::Decoder::default());
                    debug!("receiving sync parameters");
                    let Some(params) = dec
                        .try_next()
                        .await
                        .context("failed to receive sync parameters")?
                    else {
                        bail!("incomplete sync parameters")
                    };
                    trace!("received sync parameters");
                    let rx = dec.decoder_mut().take_deferred();
                    let span = Span::current();
                    Ok((
                        cx,
                        params,
                        rx.map(|f| f(dec.into_inner().into(), Vec::with_capacity(8))),
                        move |results| {
                            Box::pin(
                                async {
                                    let mut enc =
                                        FramedWrite::new(outgoing, Results::Encoder::default());
                                    debug!("transmitting sync results");
                                    enc.send(results)
                                        .await
                                        .context("failed to transmit synchronous results")?;
                                    let tx = enc.encoder_mut().take_deferred();
                                    let mut outgoing = enc.into_inner();
                                    outgoing
                                        .shutdown()
                                        .await
                                        .context("failed to shutdown synchronous return channel")?;
                                    if let Some(tx) = tx {
                                        debug!("transmitting async results");
                                        tx(outgoing.into(), Vec::with_capacity(8))
                                            .await
                                            .context("failed to write async results")?;
                                    }
                                    Ok(())
                                }
                                .instrument(span),
                            ) as Pin<_>
                        },
                    ))
                }
                .instrument(span.clone())
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::{stream, StreamExt as _};

    use super::*;

    #[allow(unused)]
    async fn call_invoke<T: Invoke>(
        cx: T::Context,
        i: &T,
        paths: Arc<[Arc<[Option<usize>]>]>,
    ) -> anyhow::Result<(T::Outgoing, T::Incoming)> {
        i.invoke(cx, "foo", "bar", Bytes::default(), &paths).await
    }

    #[allow(unused)]
    async fn call_serve<T: Serve>(
        s: &T,
    ) -> anyhow::Result<Vec<(T::Context, T::Outgoing, T::Incoming)>> {
        let st = stream::empty()
            .chain(s.serve("foo", "bar", [[Some(42), None]]).await.unwrap())
            .chain(s.serve("foo", "bar", vec![[Some(42), None]]).await.unwrap())
            .chain(s.serve("foo", "bar", [vec![Some(42), None]]).await.unwrap())
            .chain({
                let paths: Arc<[Arc<[Option<usize>]>]> = Arc::from([Arc::from([Some(42), None])]);
                s.serve("foo", "bar", paths).await.unwrap()
            })
            .chain({
                let paths: Arc<[_]> = Arc::from(vec![vec![Some(42), None]]);
                s.serve("foo", "bar", paths).await.unwrap()
            })
            .chain({
                let paths: Arc<[_]> = Arc::from([vec![Some(42), None]]);
                s.serve("foo", "bar", paths).await.unwrap()
            })
            .chain({
                let paths: Arc<[_]> = Arc::from([[Some(42), None]]);
                s.serve("foo", "bar", paths).await.unwrap()
            });
        tokio::spawn(async move { st.try_collect().await })
            .await
            .unwrap()
    }
}
