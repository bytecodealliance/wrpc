#![allow(clippy::type_complexity)]

use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::{SinkExt as _, Stream, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::{Encoder as _, FramedRead, FramedWrite};

#[cfg(feature = "frame")]
pub mod frame;
#[cfg(feature = "frame")]
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, FrameRef};

mod value;
pub use value::*;

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
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync;

    /// Incoming multiplexed byte stream
    type Incoming: AsyncRead + Index<Self::Incoming> + Send + Sync;

    /// Invoke function `func` on instance `instance`
    fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = anyhow::Result<Invocation<Self::Outgoing, Self::Incoming, Self::Session>>,
    > + Send;

    /// Invoke function `func` on instance `instance` using typed `Params` and `Returns`
    fn invoke_values<Params, Returns>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = anyhow::Result<(
            Returns,
            impl Future<Output = anyhow::Result<Result<(), String>>>,
        )>,
    > + Send
    where
        Self::Outgoing: 'static,
        Self::Incoming: Unpin,
        ValueEncoder<Self::Outgoing>: tokio_util::codec::Encoder<Params>,
        ValueDecoder<Self::Incoming, Returns>: tokio_util::codec::Decoder<Item = Returns>,
        Params: Send,
        Returns: Decode,
        <ValueEncoder<Self::Outgoing> as tokio_util::codec::Encoder<Params>>::Error:
            std::error::Error + Send + Sync + 'static,
        <ValueDecoder<Self::Incoming, Returns> as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let mut buf = BytesMut::default();
            let mut enc = ValueEncoder::<Self::Outgoing>::default();
            enc.encode(params, &mut buf)
                .context("failed to encode parameters")?;
            let Invocation {
                outgoing,
                incoming,
                session,
            } = self
                .invoke(cx, instance, func, buf.freeze(), paths)
                .await
                .context("failed to invoke function")?;
            let tx = tokio::spawn(async {
                enc.write_deferred(outgoing)
                    .await
                    .context("failed to write async values")
            });
            let mut dec = FramedRead::new(
                incoming,
                ValueDecoder::<Self::Incoming, Returns>::new(Vec::default()),
            );
            let Some(returns) = dec
                .try_next()
                .await
                .context("failed to decode return values")?
            else {
                bail!("incomplete returns")
            };
            let rx = dec.decoder_mut().take_deferred();
            Ok((returns, async {
                if let Some(rx) = rx {
                    try_join!(
                        async {
                            rx(dec.into_inner())
                                .await
                                .context("reading async return values failed")
                        },
                        async { tx.await.context("writing async parameters failed")? }
                    )?;
                } else {
                    tx.await.context("writing async parameters failed")??;
                };
                session
                    .finish(Ok(()))
                    .await
                    .context("failed to finish session")
            }))
        }
    }
}

/// Server-side handle to a wRPC transport
pub trait Serve {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Transport-specific session used for lifetime tracking and error reporting
    type Session: Session + Send + Sync;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync;

    /// Incoming multiplexed byte stream
    type Incoming: AsyncRead + Index<Self::Incoming> + Send + Sync;

    /// Serve function `func` from instance `instance`
    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Invocation<Self::Outgoing, Self::Incoming, Self::Session>,
                )>,
            >,
        >,
    > + Send;

    /// Serve function `func` from instance `instance` using typed `Params` and `Returns`
    fn serve_values<Params, Returns>(
        &self,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Params,
                    Option<impl Future<Output = std::io::Result<()>> + Sync + Send + Unpin>,
                    impl FnOnce(
                        Result<Returns, Arc<str>>,
                    ) -> Pin<
                        Box<dyn Future<Output = anyhow::Result<Result<(), String>>> + Sync + Send>,
                    >,
                )>,
            >,
        >,
    > + Send
    where
        Self: Sync,
        Self::Incoming: Unpin,
        Self::Outgoing: Unpin + 'static,
        Self::Session: 'static,
        Params: Decode,
        Returns: Send + Sync + 'static,
        ValueEncoder<Self::Outgoing>: tokio_util::codec::Encoder<Returns>,
        ValueDecoder<Self::Incoming, Params>: tokio_util::codec::Decoder<Item = Params>,
        <ValueEncoder<Self::Outgoing> as tokio_util::codec::Encoder<Returns>>::Error:
            std::error::Error + Send + Sync + 'static,
        <ValueDecoder<Self::Incoming, Params> as tokio_util::codec::Decoder>::Error:
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
                )| async {
                    let mut dec = FramedRead::new(
                        incoming,
                        ValueDecoder::<Self::Incoming, Params>::new(Vec::default()),
                    );
                    let Some(params) = dec
                        .try_next()
                        .await
                        .context("failed to decode parameters")?
                    else {
                        bail!("incomplete parameters")
                    };
                    let rx = dec.decoder_mut().take_deferred();
                    Ok((
                        cx,
                        params,
                        rx.map(|f| f(dec.into_inner())),
                        |returns: Result<_, Arc<str>>| {
                            Box::pin(async move {
                                match returns {
                                    Ok(returns) => {
                                        let mut enc = FramedWrite::<
                                            Self::Outgoing,
                                            ValueEncoder<Self::Outgoing>,
                                        >::new(
                                            outgoing,
                                            ValueEncoder::<Self::Outgoing>::default(),
                                        );
                                        enc.send(returns)
                                            .await
                                            .context("failed to write return values")?;
                                        if let Some(tx) = enc.encoder_mut().take_deferred() {
                                            tx(enc.into_inner())
                                                .await
                                                .context("failed to write async return values")?;
                                        }
                                        session
                                            .finish(Ok(()))
                                            .await
                                            .context("failed to finish session")
                                    }
                                    Err(err) => session
                                        .finish(Err(&err))
                                        .await
                                        .context("failed to finish session"),
                                }
                            }) as Pin<_>
                        },
                    ))
                },
            ))
        }
    }
}
