//! wRPC transport client handle

use core::future::Future;
use core::mem;
use core::pin::pin;
use core::time::Duration;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::TryStreamExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio::{select, try_join};
use tokio_util::codec::{Encoder as _, FramedRead};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::{Incoming, Outgoing};
use crate::{Deferred as _, TupleDecode, TupleEncode};

/// Client-side handle to a wRPC transport
///
/// Invocations are always multiplexed over the wRPC framing layer, so the outgoing and
/// incoming byte streams are the framed [`Outgoing`] and [`Incoming`] streams regardless of
/// the underlying transport.
pub trait Invoke: Send + Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Invoke function `func` on instance `instance`
    fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Outgoing, Incoming)>> + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync;
}

/// Wrapper struct returned by [`InvokeExt::timeout`]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeout<'a, T: ?Sized> {
    /// Inner [Invoke]
    pub inner: &'a T,
    /// Invocation timeout
    pub timeout: Duration,
}

impl<T: Invoke> Invoke for Timeout<'_, T> {
    type Context = T::Context;

    #[instrument(level = "trace", skip(self, cx, params, paths))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        tokio::time::timeout(
            self.timeout,
            self.inner.invoke(cx, instance, func, params, paths),
        )
        .await
        .context("invocation timed out")?
    }
}

/// Wrapper struct returned by [`InvokeExt::timeout_owned`]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutOwned<T> {
    /// Inner [Invoke]
    pub inner: T,
    /// Invocation timeout
    pub timeout: Duration,
}

impl<T: Invoke> Invoke for TimeoutOwned<T> {
    type Context = T::Context;

    #[instrument(level = "trace", skip(self, cx, params, paths))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        self.inner
            .timeout(self.timeout)
            .invoke(cx, instance, func, params, paths)
            .await
    }
}

/// Extension trait for [Invoke]
pub trait InvokeExt: Invoke {
    /// Invoke function `func` on instance `instance` using typed `Params` and `Results`
    #[instrument(level = "trace", skip(self, cx, params, paths))]
    fn invoke_values<P, Params, Results, Paths>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: Paths,
    ) -> impl Future<
        Output = anyhow::Result<(
            Results,
            Option<
                impl Future<Output = anyhow::Result<()>>
                + Send
                + 'static
                + use<Self, P, Params, Results, Paths>,
            >,
        )>,
    > + Send
    where
        Paths: AsRef<[P]> + Send,
        P: AsRef<[Option<usize>]> + Send + Sync,
        Params: TupleEncode + Send,
        Results: TupleDecode + Send,
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
            trace!("shutdown synchronous parameter channel");
            outgoing
                .shutdown()
                .await
                .context("failed to shutdown synchronous parameter channel")?;
            let mut tx = enc.take_deferred().map(|tx| {
                tokio::spawn(
                    async {
                        debug!("transmitting async parameters");
                        tx(outgoing, Vec::default())
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
            let results = match tx.take() {
                Some(mut fut) => {
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
                }
                _ => results.await?,
            };
            trace!("received sync results");
            let buffer = mem::take(dec.read_buffer_mut());
            let rx = dec.decoder_mut().take_deferred();
            let incoming = crate::Incoming {
                buffer,
                inner: dec.into_inner(),
            };
            Ok((
                results,
                (tx.is_some() || rx.is_some()).then_some(
                    async {
                        match (tx, rx) {
                            (Some(tx), Some(rx)) => {
                                try_join!(
                                    async {
                                        debug!("receiving async results");
                                        rx(incoming, Vec::default())
                                            .await
                                            .context("receiving async results failed")
                                    },
                                    async {
                                        tx.await.context("transmitting async parameters failed")?
                                    }
                                )?;
                            }
                            (Some(tx), None) => {
                                tx.await.context("transmitting async parameters failed")??;
                            }
                            (None, Some(rx)) => {
                                debug!("receiving async results");
                                rx(incoming, Vec::default())
                                    .await
                                    .context("receiving async results failed")?;
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    .in_current_span(),
                ),
            ))
        }
    }

    /// Invoke function `func` on instance `instance` using typed `Params` and `Results`
    /// This is like [`Self::invoke_values`], but it only results once all I/O is done
    #[instrument(level = "trace", skip_all)]
    fn invoke_values_blocking<P, Params, Results>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Params,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<Results>> + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
        Params: TupleEncode + Send,
        Results: TupleDecode + Send,
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
                trace!("awaiting I/O completion");
                io.await?;
            }
            Ok(ret)
        }
    }

    /// Returns a [`Timeout`], wrapping [Self] with an implementation of [Invoke], which will
    /// error, if call to [`Invoke::invoke`] does not return within a supplied `timeout`
    fn timeout(&self, timeout: Duration) -> Timeout<'_, Self> {
        Timeout {
            inner: self,
            timeout,
        }
    }

    /// This is like [`InvokeExt::timeout`], but moves [Self] and returns corresponding [`TimeoutOwned`]
    fn timeout_owned(self, timeout: Duration) -> TimeoutOwned<Self>
    where
        Self: Sized,
    {
        TimeoutOwned {
            inner: self,
            timeout,
        }
    }
}

impl<T: Invoke> InvokeExt for T {}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;

    use std::sync::Arc;

    use bytes::Bytes;
    use futures::{Stream, StreamExt as _};

    use super::*;

    #[allow(clippy::manual_async_fn)]
    fn invoke_values_send<T>() -> impl Future<
        Output = anyhow::Result<(
            Pin<Box<dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = String> + Send>>>> + Send>>,
        )>,
    > + Send
    where
        T: Invoke<Context = ()> + Default,
    {
        async {
            let wrpc = T::default();
            let ((r0,), _) = wrpc
                .invoke_values(
                    (),
                    "wrpc-test:integration/async",
                    "with-streams",
                    (),
                    [[None].as_slice()],
                )
                .await?;
            Ok(r0)
        }
    }

    async fn call_invoke<T: Invoke>(
        i: &T,
        cx: T::Context,
        paths: Arc<[Arc<[Option<usize>]>]>,
    ) -> anyhow::Result<(Outgoing, Incoming)> {
        i.invoke(cx, "foo", "bar", Bytes::default(), &paths).await
    }

    async fn call_invoke_async<T>() -> anyhow::Result<(Pin<Box<dyn Stream<Item = Bytes> + Send>>,)>
    where
        T: Invoke<Context = ()> + Default,
    {
        let wrpc = T::default();
        let ((r0,), _) = wrpc
            .invoke_values(
                (),
                "wrpc-test:integration/async",
                "with-streams",
                (),
                [
                    [Some(1), Some(2)].as_slice(),
                    [None].as_slice(),
                    [Some(42)].as_slice(),
                ],
            )
            .await?;
        Ok(r0)
    }

    trait Handler {
        fn foo() -> impl Future<Output = anyhow::Result<()>>;
    }

    impl<T> Handler for T
    where
        T: Invoke<Context = ()> + Default,
    {
        async fn foo() -> anyhow::Result<()> {
            let (st,) = call_invoke_async::<Self>().await?;
            st.collect::<Vec<_>>().await;
            Ok(())
        }
    }
}
