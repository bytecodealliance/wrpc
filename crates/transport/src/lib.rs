#![allow(clippy::type_complexity)]

#[cfg(feature = "frame")]
pub mod frame;

mod value;

#[cfg(feature = "frame")]
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, FrameRef};
pub use value::*;

use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::{SinkExt as _, Stream, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::{Encoder as _, FramedRead, FramedWrite};
use tracing::{debug, instrument, trace, Instrument as _};

/// `Index` implementations are capable of multiplexing underlying connections using a particular
/// structural `path`
pub trait Index<T> {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> anyhow::Result<T>;
}

/// Invocation session, which is used to track the lifetime of the data exchange and for
/// error reporting.
pub trait Session {
    /// Finish the invocation session, closing all associated communication channels and reporting
    /// a result to the peer.
    fn finish(
        self,
        res: Result<(), &str>,
    ) -> impl Future<Output = anyhow::Result<Result<(), String>>> + Send + Sync;
}

/// Invocation encapsulates either server or client-side triple of:
/// - Multiplexed outgoing byte stream
/// - Multiplexed incoming byte stream
/// - Active session, used to communicate and receive transport-layer errors
pub struct Invocation<O, I, S> {
    /// Outgoing multiplexed byte stream
    pub outgoing: O,

    /// Incoming multiplexed byte stream
    pub incoming: I,

    /// Invocation session
    pub session: S,
}

/// Client-side handle to a wRPC transport
pub trait Invoke: Send + Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Transport-specific session used for lifetime tracking and error reporting
    type Session: Session + Send + Sync;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync + 'static;

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
    ) -> impl Future<
        Output = anyhow::Result<Invocation<Self::Outgoing, Self::Incoming, Self::Session>>,
    > + Send;

    /// Invoke function `func` on instance `instance` using typed `Params` and `Returns`
    #[instrument(level = "trace", skip(self, cx, params, paths))]
    fn invoke_values<Params, Returns>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: &[impl AsRef<[Option<usize>]> + Send + Sync],
    ) -> impl Future<
        Output = anyhow::Result<(
            Returns,
            impl Future<Output = anyhow::Result<Result<(), String>>>,
        )>,
    > + Send
    where
        Params: Encode<Self::Outgoing> + Send,
        Returns: Decode<Self::Incoming>,
        <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
            std::error::Error + Send + Sync + 'static,
        <Returns::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let mut buf = BytesMut::default();
            let mut enc = Params::Encoder::default();
            trace!("encoding parameters");
            enc.encode(params, &mut buf)
                .context("failed to encode parameters")?;
            debug!("invoking function");
            let Invocation {
                outgoing,
                incoming,
                session,
            } = self
                .invoke(cx, instance, func, buf.freeze(), paths)
                .await
                .context("failed to invoke function")?;
            let tx = enc.take_deferred().map(|tx| {
                tokio::spawn(
                    async {
                        trace!("writing async parameters");
                        tx(outgoing.into(), Vec::with_capacity(8))
                            .await
                            .context("failed to write async parameters")
                    }
                    .in_current_span(),
                )
            });

            let mut dec = FramedRead::new(incoming, Returns::Decoder::default());
            debug!("receiving sync returns");
            let Some(returns) = dec
                .try_next()
                .await
                .context("failed to receive sync returns")?
            else {
                bail!("incomplete returns")
            };
            let rx = dec.decoder_mut().take_deferred();
            Ok((returns, async {
                if let Some(rx) = rx {
                    try_join!(
                        async {
                            debug!("receiving async returns");
                            rx(dec.into_inner().into(), Vec::with_capacity(8))
                                .await
                                .context("receiving async returns failed")
                        },
                        async {
                            if let Some(tx) = tx {
                                tx.await.context("writing async parameters failed")?
                            } else {
                                Ok(())
                            }
                        }
                    )?;
                } else if let Some(tx) = tx {
                    tx.await.context("writing async parameters failed")??;
                };
                debug!("finishing session");
                session
                    .finish(Ok(()))
                    .await
                    .context("failed to finish session")
            }))
        }
    }
}

/// Server-side handle to a wRPC transport
pub trait Serve: Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Transport-specific session used for lifetime tracking and error reporting
    type Session: Session + Send + Sync + 'static;

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
            impl Stream<
                    Item = anyhow::Result<(
                        Self::Context,
                        Invocation<Self::Outgoing, Self::Incoming, Self::Session>,
                    )>,
                > + Send
                + 'static,
        >,
    > + Send;

    /// Serve function `func` from instance `instance` using typed `Params` and `Returns`
    #[instrument(level = "trace", skip(self, paths))]
    fn serve_values<P, Params, Returns>(
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
                            Result<Returns, Arc<str>>,
                        ) -> Pin<
                            Box<
                                dyn Future<Output = anyhow::Result<Result<(), String>>>
                                    + Sync
                                    + Send,
                            >,
                        >,
                    )>,
                > + Send
                + 'static,
        >,
    > + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync + 'static,
        Params: Decode<Self::Incoming> + Send + Sync + 'static,
        Returns: Encode<Self::Outgoing> + Send + Sync + 'static,
        <Params::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
        <Returns::Encoder as tokio_util::codec::Encoder<Returns>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let invocations = self.serve(instance, func, paths).await?;
            Ok(invocations.and_then(
                |(
                    cx,
                    Invocation {
                        outgoing,
                        incoming,
                        session,
                    },
                )| {
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
                        Ok((
                            cx,
                            params,
                            rx.map(|f| f(dec.into_inner().into(), Vec::with_capacity(8))),
                            |returns: Result<_, Arc<str>>| {
                                Box::pin(async move {
                                    match returns {
                                        Ok(returns) => {
                                            let mut enc = FramedWrite::new(
                                                outgoing,
                                                Returns::Encoder::default(),
                                            );
                                            debug!("transmitting sync returns");
                                            enc.send(returns).await.context(
                                                "failed to transmit synchronous returns",
                                            )?;
                                            if let Some(tx) = enc.encoder_mut().take_deferred() {
                                                debug!("transmitting async returns");
                                                tx(enc.into_inner().into(), Vec::with_capacity(8))
                                                    .await
                                                    .context("failed to write async returns")?;
                                            }
                                            debug!("finishing session with success");
                                            session
                                                .finish(Ok(()))
                                                .await
                                                .context("failed to finish session")
                                        }
                                        Err(err) => {
                                            debug!(?err, "finishing session with an error");
                                            session
                                                .finish(Err(&err))
                                                .await
                                                .context("failed to finish session")
                                        }
                                    }
                                }) as Pin<_>
                            },
                        ))
                    }
                },
            ))
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
    ) -> anyhow::Result<Invocation<T::Outgoing, T::Incoming, T::Session>> {
        i.invoke(cx, "foo", "bar", Bytes::default(), &paths).await
    }

    #[allow(unused)]
    async fn call_serve<T: Serve>(
        s: &T,
    ) -> anyhow::Result<Vec<(T::Context, Invocation<T::Outgoing, T::Incoming, T::Session>)>> {
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
