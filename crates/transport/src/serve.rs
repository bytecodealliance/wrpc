//! wRPC transport server handle

use core::pin::Pin;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::BytesMut;
use futures::{Stream, TryStreamExt as _};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::{Decoder as _, Encoder as _, FramedParts, FramedRead};
use tracing::{Instrument as _, Span, debug, instrument, trace};

use crate::frame::conn::mux;
use crate::frame::{Incoming, Outgoing};
use crate::{Deferred as _, TupleDecode, TupleEncode, error_trailing_data};

/// Server-side handle to a wRPC transport
///
/// An invocation is a single bidirectional byte stream, which [`ServeExt`] multiplexes using
/// the wRPC [framing](crate::frame) protocol.
pub trait Serve: Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Outgoing byte stream
    type Outgoing: AsyncWrite + Send + Unpin + 'static;

    /// Incoming byte stream
    type Incoming: AsyncBufRead + Send + Unpin + 'static;

    /// Serve function `func` from instance `instance`
    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
            + Send
            + 'static
            + use<Self>,
        >,
    > + Send;
}

/// Extension trait for [Serve]
pub trait ServeExt: Serve {
    /// Serve function `func` from instance `instance`, yielding the framed streams of each
    /// invocation, which multiplex the async `paths`
    #[instrument(level = "trace", skip(self, paths))]
    fn serve_framed(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Outgoing<Self::Outgoing>,
                    Incoming<Self::Incoming>,
                )>,
            >
            + Send
            + 'static
            + use<Self>,
        >,
    > + Send {
        async {
            let invocations = self.serve(instance, func, Arc::clone(&paths)).await?;
            Ok(invocations.map_ok(move |(cx, tx, rx)| {
                let (tx, rx) = mux(tx, rx, paths.iter());
                (cx, tx, rx)
            }))
        }
    }

    /// Serve function `func` from instance `instance` using typed `Params` and `Results`
    ///
    /// Each invocation is yielded along with its synchronous parameters, a future receiving the
    /// asynchronous ones, if there are any, and a function transmitting its results, which
    /// resolves once the invocation ends. Nothing is received unless it is polled.
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
        Params: TupleDecode<Incoming<Self::Incoming>> + Send + 'static,
        Results: TupleEncode<Outgoing<Self::Outgoing>> + Send + 'static,
        <Params::Decoder as tokio_util::codec::Decoder>::Error:
            core::error::Error + Send + Sync + 'static,
        <Results::Encoder as tokio_util::codec::Encoder<Results>>::Error:
            core::error::Error + Send + Sync + 'static,
    {
        let span = Span::current();
        async {
            let invocations = self.serve_framed(instance, func, paths).await?;
            Ok(invocations.and_then(move |(cx, mut outgoing, incoming)| {
                async {
                    let closed = incoming.closed();
                    let mut dec = FramedRead::new(incoming, Params::Decoder::default());
                    debug!("receiving sync parameters");
                    let params = match dec
                        .decoder_mut()
                        .decode(&mut BytesMut::default())
                        .context("failed to decode sync parameters")?
                    {
                        Some(params) => params,
                        None => {
                            let params = dec
                                .try_next()
                                .await
                                .context("failed to receive sync parameters")?;
                            params.context("incomplete sync parameters")?
                        }
                    };
                    trace!("received sync parameters");
                    let rx = dec.decoder_mut().take_deferred();
                    let FramedParts {
                        io: incoming,
                        read_buf,
                        ..
                    } = dec.into_parts();
                    if !read_buf.is_empty() {
                        return Err(error_trailing_data())
                            .context("failed to receive sync parameters");
                    }
                    let span = Span::current();
                    Ok((cx, params, rx.map(|f| f(incoming)), move |results| {
                        Box::pin(
                            async move {
                                let mut enc = Results::Encoder::default();
                                let mut buf = BytesMut::default();
                                enc.encode(results, &mut buf)
                                    .context("failed to encode synchronous results")?;
                                debug!("transmitting sync results");
                                outgoing
                                    .write_all(&buf)
                                    .await
                                    .context("failed to transmit synchronous results")?;
                                outgoing
                                    .shutdown()
                                    .await
                                    .context("failed to close synchronous results")?;
                                let tx = enc.take_deferred();
                                if let Some(tx) = tx {
                                    debug!("transmitting async results");
                                    tx(outgoing)
                                        .await
                                        .context("failed to write async results")?;
                                } else {
                                    drop(outgoing);
                                }
                                closed.await.context("invocation failed")?;
                                Ok(())
                            }
                            .instrument(span),
                        ) as Pin<_>
                    }))
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
    use futures::{StreamExt as _, stream};

    use super::*;

    async fn call_serve<T: Serve>(
        s: &T,
    ) -> anyhow::Result<Vec<(T::Context, T::Outgoing, T::Incoming)>> {
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
    > {
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
    > {
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
