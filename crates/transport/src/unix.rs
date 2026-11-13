//! Unix domain socket transport

use core::borrow::Borrow;
use core::task::{Poll, ready};

use std::path::{Path, PathBuf};

use futures::{Stream, stream};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf, SocketAddr};
use tokio::net::{UnixListener, UnixStream};
use tracing::instrument;

use crate::Connect;

/// Returns the stream of invocations accepted on `lis`, each carried by one connection, with
/// the peer address as the context
pub fn incoming(
    lis: impl Borrow<UnixListener>,
) -> impl Stream<Item = std::io::Result<(SocketAddr, OwnedWriteHalf, OwnedReadHalf)>> {
    stream::poll_fn(move |cx| {
        let (stream, addr) = ready!(lis.borrow().poll_accept(cx))?;
        let (rx, tx) = stream.into_split();
        Poll::Ready(Some(Ok((addr, tx, rx))))
    })
}

/// [Connect] implementation in terms of a single [`UnixStream`].
///
/// [`Connect::connect`] can only be called at most once on [Oneshot],
/// repeated calls will return an error.
pub type Oneshot = crate::Oneshot<OwnedReadHalf, OwnedWriteHalf>;

impl Oneshot {
    /// Creates a pair of connected [Oneshot] using [`UnixStream::pair`].
    pub fn unix_pair() -> std::io::Result<(Oneshot, UnixStream)> {
        let (clt, srv) = UnixStream::pair()?;
        Ok((clt.into(), srv))
    }
}

impl From<UnixStream> for Oneshot {
    fn from(stream: UnixStream) -> Self {
        stream.into_split().into()
    }
}

/// [Connect] implementation of a Unix domain socket transport
#[derive(Clone, Debug)]
pub struct Client<T>(T);

impl From<PathBuf> for Client<PathBuf> {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl<'a> From<&'a Path> for Client<&'a Path> {
    fn from(path: &'a Path) -> Self {
        Self(path)
    }
}

impl From<std::os::unix::net::SocketAddr> for Client<SocketAddr> {
    fn from(addr: std::os::unix::net::SocketAddr) -> Self {
        Self(addr.into())
    }
}

impl<'a> From<&'a SocketAddr> for Client<&'a SocketAddr> {
    fn from(addr: &'a SocketAddr) -> Self {
        Self(addr)
    }
}

impl From<SocketAddr> for Client<SocketAddr> {
    fn from(addr: SocketAddr) -> Self {
        Self(addr)
    }
}

impl Connect for Client<&Path> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    async fn connect(
        &self,
        (): Self::Context,
    ) -> std::io::Result<(Self::Outgoing, Self::Incoming)> {
        let stream = UnixStream::connect(self.0).await?;
        let (rx, tx) = stream.into_split();
        Ok((tx, rx))
    }
}

impl Connect for Client<PathBuf> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    async fn connect(
        &self,
        (): Self::Context,
    ) -> std::io::Result<(Self::Outgoing, Self::Incoming)> {
        Client(self.0.as_path()).connect(()).await
    }
}

impl Connect for Client<&SocketAddr> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    async fn connect(
        &self,
        (): Self::Context,
    ) -> std::io::Result<(Self::Outgoing, Self::Incoming)> {
        let stream = UnixStream::connect_addr(self.0).await?;
        let (rx, tx) = stream.into_split();
        Ok((tx, rx))
    }
}

impl Connect for Client<SocketAddr> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    async fn connect(
        &self,
        (): Self::Context,
    ) -> std::io::Result<(Self::Outgoing, Self::Incoming)> {
        Client(&self.0).connect(()).await
    }
}
