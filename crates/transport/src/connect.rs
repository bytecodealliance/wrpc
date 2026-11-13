//! wRPC transport connector

use core::time::Duration;

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tracing::instrument;

/// Client-side connector of a wRPC transport
///
/// An invocation is a single bidirectional byte stream, which [`invoke`](crate::invoke())
/// multiplexes using the wRPC [framing](crate::frame) protocol.
pub trait Connect: Send + Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync;

    /// Outgoing byte stream
    type Outgoing: AsyncWrite + Send + Unpin + 'static;

    /// Incoming byte stream
    type Incoming: AsyncRead + Send + Unpin + 'static;

    /// Error returned when establishing the byte streams fails
    type Error: core::error::Error + Send + Sync + 'static;

    /// Establishes the byte streams of a new invocation
    fn connect(
        &self,
        cx: Self::Context,
    ) -> impl Future<Output = Result<(Self::Outgoing, Self::Incoming), Self::Error>> + Send;
}

impl<T: Connect + ?Sized> Connect for &T {
    type Context = T::Context;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;
    type Error = T::Error;

    fn connect(
        &self,
        cx: Self::Context,
    ) -> impl Future<Output = Result<(Self::Outgoing, Self::Incoming), Self::Error>> + Send {
        T::connect(self, cx)
    }
}

impl<T: Connect> Connect for Arc<T> {
    type Context = T::Context;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;
    type Error = T::Error;

    fn connect(
        &self,
        cx: Self::Context,
    ) -> impl Future<Output = Result<(Self::Outgoing, Self::Incoming), Self::Error>> + Send {
        T::connect(self, cx)
    }
}

/// Error returned by [Timeout]
#[derive(Debug)]
pub enum TimeoutError<E> {
    /// Establishing the byte streams did not complete within the timeout
    Elapsed,
    /// Establishing the byte streams failed
    Connect(E),
}

impl<E: core::fmt::Display> core::fmt::Display for TimeoutError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Elapsed => f.write_str("connection timed out"),
            Self::Connect(err) => err.fmt(f),
        }
    }
}

impl<E: core::error::Error + 'static> core::error::Error for TimeoutError<E> {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Elapsed => None,
            Self::Connect(err) => Some(err),
        }
    }
}

/// Wrapper struct returned by [`ConnectExt::timeout`]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeout<T> {
    /// Inner [Connect]
    pub inner: T,
    /// Connection timeout
    pub timeout: Duration,
}

impl<T: Connect> Connect for Timeout<T> {
    type Context = T::Context;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;
    type Error = TimeoutError<T::Error>;

    #[instrument(level = "trace", skip(self, cx))]
    async fn connect(
        &self,
        cx: Self::Context,
    ) -> Result<(Self::Outgoing, Self::Incoming), Self::Error> {
        match tokio::time::timeout(self.timeout, self.inner.connect(cx)).await {
            Ok(Ok(streams)) => Ok(streams),
            Ok(Err(err)) => Err(TimeoutError::Connect(err)),
            Err(..) => Err(TimeoutError::Elapsed),
        }
    }
}

/// Extension trait for [Connect]
pub trait ConnectExt: Connect + Sized {
    /// Returns a [`Timeout`], wrapping [Self] with an implementation of [Connect], which fails,
    /// if [`Connect::connect`] does not return within `timeout`
    fn timeout(self, timeout: Duration) -> Timeout<Self> {
        Timeout {
            inner: self,
            timeout,
        }
    }
}

impl<T: Connect> ConnectExt for T {}
