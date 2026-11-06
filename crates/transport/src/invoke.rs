//! wRPC transport client handle

use core::future::poll_fn;
use core::pin::Pin;
use core::task::Poll;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::StreamExt as _;
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt as _};
use tokio::try_join;
use tokio_util::codec::{Encoder as _, FramedParts, FramedRead};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::conn::mux;
use crate::frame::{Incoming, Outgoing};
use crate::{Deferred as _, TupleDecode, TupleEncode, error_trailing_data};

/// Client-side handle to a wRPC transport
///
/// An invocation is a single bidirectional byte stream, which [`InvokeExt`] multiplexes using
/// the wRPC [framing](crate::frame) protocol.
pub trait Invoke: Send + Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Outgoing byte stream
    type Outgoing: AsyncWrite + Send + Unpin + 'static;

    /// Incoming byte stream
    type Incoming: AsyncBufRead + Send + Unpin + 'static;

    /// Invoke function `func` on instance `instance`, returning the streams of the invocation
    /// with the invocation header and `params` written to the outgoing one as the first data
    /// of the root path, which is left open.
    fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Self::Outgoing, Self::Incoming)>> + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync;
}

impl<T: Invoke> Invoke for Arc<T> {
    type Context = T::Context;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Self::Outgoing, Self::Incoming)>> + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        T::invoke(self, cx, instance, func, params, paths)
    }
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
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    #[instrument(level = "trace", skip(self, cx, params, paths))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
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
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    #[instrument(level = "trace", skip(self, cx, params, paths))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
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
    /// Invoke function `func` on instance `instance`, returning the framed streams of the
    /// invocation, which multiplex the async `paths`
    #[instrument(level = "trace", skip(self, cx, params, paths))]
    fn invoke_framed<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Outgoing<Self::Outgoing>, Incoming<Self::Incoming>)>> + Send
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        async {
            let (tx, rx) = self
                .invoke(cx, instance, func, params, paths.as_ref())
                .await?;
            Ok(mux(tx, rx, paths.as_ref()))
        }
    }

    /// Invoke function `func` on instance `instance` using typed `Params` and `Results`
    ///
    /// Returns the synchronous results along with a future, which completes once the
    /// asynchronous parameters are transmitted, the asynchronous results are received, if there
    /// are any, and the invocation ends. Nothing is transmitted or received unless it is polled.
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
            impl Future<Output = anyhow::Result<()>>
            + Send
            + 'static
            + use<Self, P, Params, Results, Paths>,
        )>,
    > + Send
    where
        Paths: AsRef<[P]> + Send,
        P: AsRef<[Option<usize>]> + Send + Sync,
        Params: TupleEncode<Outgoing<Self::Outgoing>> + Send,
        Results: TupleDecode<Incoming<Self::Incoming>> + Send,
        <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
            core::error::Error + Send + Sync + 'static,
        <Results::Decoder as tokio_util::codec::Decoder>::Error:
            core::error::Error + Send + Sync + 'static,
    {
        async {
            let mut buf = BytesMut::default();
            let mut enc = Params::Encoder::default();
            trace!("encoding parameters");
            enc.encode(params, &mut buf)
                .context("failed to encode parameters")?;
            debug!("invoking function");
            let (mut outgoing, incoming) = self
                .invoke_framed(cx, instance, func, buf.freeze(), paths)
                .await
                .context("failed to invoke function")?;
            let tx = enc.take_deferred();
            let mut tx: Option<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>> =
                Some(Box::pin(
                    async move {
                        outgoing
                            .shutdown()
                            .await
                            .context("failed to close parameters")?;
                        let Some(tx) = tx else {
                            drop(outgoing);
                            return Ok(());
                        };
                        debug!("transmitting async parameters");
                        tx(outgoing)
                            .await
                            .context("failed to write async parameters")
                    }
                    .in_current_span(),
                ));
            let closed = incoming.closed();
            let mut dec = FramedRead::new(incoming, Results::Decoder::default());
            debug!("receiving sync results");
            let results = poll_fn(|cx| {
                match dec.poll_next_unpin(cx) {
                    Poll::Ready(Some(Ok(results))) => return Poll::Ready(Ok(results)),
                    Poll::Ready(Some(Err(err))) => {
                        return Poll::Ready(Err(err).context("failed to receive sync results"));
                    }
                    Poll::Ready(None) => {
                        return Poll::Ready(Err(anyhow::anyhow!("incomplete results")));
                    }
                    Poll::Pending => {}
                }
                if let Some(fut) = &mut tx
                    && let Poll::Ready(res) = fut.as_mut().poll(cx)
                {
                    tx = None;
                    res?;
                }
                Poll::Pending
            })
            .await?;
            trace!("received sync results");
            let rx = dec.decoder_mut().take_deferred();
            let FramedParts {
                io: incoming,
                read_buf,
                ..
            } = dec.into_parts();
            if !read_buf.is_empty() {
                return Err(error_trailing_data()).context("failed to receive sync results");
            }
            Ok((
                results,
                async move {
                    try_join!(
                        async {
                            let Some(rx) = rx else {
                                drop(incoming);
                                return Ok(());
                            };
                            debug!("receiving async results");
                            rx(incoming).await.context("receiving async results failed")
                        },
                        async {
                            if let Some(tx) = tx {
                                tx.await?;
                            }
                            Ok(())
                        },
                    )?;
                    closed.await.context("invocation failed")?;
                    Ok(())
                }
                .in_current_span(),
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
        Params: TupleEncode<Outgoing<Self::Outgoing>> + Send,
        Results: TupleDecode<Incoming<Self::Incoming>> + Send,
        <Params::Encoder as tokio_util::codec::Encoder<Params>>::Error:
            core::error::Error + Send + Sync + 'static,
        <Results::Decoder as tokio_util::codec::Decoder>::Error:
            core::error::Error + Send + Sync + 'static,
    {
        async {
            let (ret, io) = self
                .invoke_values(cx, instance, func, params, paths)
                .await?;
            trace!("awaiting I/O completion");
            io.await?;
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
    use core::pin::Pin;

    use std::sync::Arc;

    use futures::Stream;

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
    ) -> anyhow::Result<(T::Outgoing, T::Incoming)> {
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
