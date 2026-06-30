//! wRPC transport server handle

use core::future::Future;
use core::mem;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{Context as _, bail};
use futures::{SinkExt as _, Stream, TryStreamExt as _};
use tokio::io::AsyncWriteExt as _;
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{Instrument as _, Span, debug, instrument, trace};

use crate::frame::{Incoming, Outgoing};
use crate::{BufferedIncoming, Deferred as _, TupleDecode, TupleEncode};

/// Server-side handle to a wRPC transport
///
/// Invocations are always multiplexed over the wRPC framing layer, so the outgoing and
/// incoming byte streams are the framed [`Outgoing`] and [`Incoming`] streams regardless of
/// the underlying transport.
pub trait Serve: Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Serve function `func` from instance `instance`
    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<Item = anyhow::Result<(Self::Context, Outgoing, Incoming)>>
            + Send
            + 'static
            + use<Self>,
        >,
    > + Send;
}

/// Extension trait for [Serve]
pub trait ServeExt: Serve {
    /// Serve function `func` from instance `instance` using typed `Params` and `Results`
    #[instrument(level = "trace", skip(self, paths))]
    fn serve_values<Params, Results>(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Params,
                    Option<
                        impl Future<Output = std::io::Result<()>>
                        + Send
                        + Unpin
                        + 'static
                        + use<Self, Params, Results>,
                    >,
                    impl FnOnce(
                        Results,
                    )
                        -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>
                    + Send
                    + 'static
                    + use<Self, Params, Results>,
                )>,
            >
            + Send
            + 'static
            + use<Self, Params, Results>,
        >,
    > + Send
    where
        Params: TupleDecode + Send + 'static,
        Results: TupleEncode + Send + 'static,
        <Params::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
        <Results::Encoder as tokio_util::codec::Encoder<Results>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        let span = Span::current();
        async {
            let invocations = self.serve(instance, func, paths).await?;
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
                    let buffer = mem::take(dec.read_buffer_mut());
                    let span = Span::current();
                    Ok((
                        cx,
                        params,
                        rx.map(|f| {
                            f(
                                BufferedIncoming {
                                    buffer,
                                    inner: dec.into_inner(),
                                },
                                Vec::default(),
                            )
                        }),
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
                                        tx(outgoing, Vec::default())
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

impl<T: Serve> ServeExt for T {}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures::{StreamExt as _, TryStreamExt as _, stream};

    use crate::Captures;

    use super::*;

    async fn call_serve<T: Serve>(s: &T) -> anyhow::Result<Vec<(T::Context, Outgoing, Incoming)>> {
        let st = stream::empty()
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    Arc::from([Box::from([Some(42), None]), Box::from([None])]),
                )
                .await
                .unwrap()
            })
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    Arc::from([Box::from([Some(42), None]), Box::from([None])]),
                )
                .await
                .unwrap()
            })
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    Arc::from([Box::from([Some(42), None]), Box::from([None])]),
                )
                .await
                .unwrap()
            });
        tokio::spawn(async move { st.try_collect().await })
            .await
            .unwrap()
    }

    fn serve_lifetime<T: Serve>(
        s: &T,
    ) -> impl Future<
        Output = anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<T::Context>> + 'static>>>,
    > + Captures<'_> {
        let fut = s.serve(
            "foo",
            "bar",
            Arc::from([Box::from([Some(42), None]), Box::from([None])]),
        );
        async move {
            let st = fut.await.unwrap();
            Ok(Box::pin(st.and_then(|(cx, _, _)| async { Ok(cx) }))
                as Pin<Box<dyn Stream<Item = _>>>)
        }
    }

    fn serve_values_lifetime<T: Serve>(
        s: &T,
    ) -> impl Future<
        Output = anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<T::Context>> + 'static>>>,
    > + crate::Captures<'_> {
        let fut = s.serve_values::<(Bytes,), (Bytes,)>(
            "foo",
            "bar",
            Arc::from([Box::from([Some(42), None]), Box::from([None])]),
        );
        async move {
            let st = fut.await.unwrap();
            Ok(Box::pin(st.and_then(|(cx, _, _, tx)| async {
                tx((Bytes::from("test"),)).await.unwrap();
                Ok(cx)
            })) as Pin<Box<dyn Stream<Item = _>>>)
        }
    }
}
