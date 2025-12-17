//! Unix domain socket transport

use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf, SocketAddr};
use tokio::net::{UnixListener, UnixStream};
use tracing::instrument;

use crate::frame::{invoke, Accept, Incoming, Outgoing};
use crate::Invoke;

/// [Invoke] and [Accept] implementation in terms of a single [`UnixStream`].
///
/// Either [`Invoke::invoke`] or [`Accept::accept`] can only be called at most once
/// on [Oneshot], repeated calls with return an error
pub type Oneshot = super::Oneshot<OwnedReadHalf, OwnedWriteHalf>;

impl Oneshot {
    /// Creates a pair of connected [Oneshot] using [UnixStream::pair].
    pub fn unix_pair() -> std::io::Result<(Oneshot, Oneshot)> {
        let (a, b) = UnixStream::pair()?;
        Ok((a.into(), b.into()))
    }
}

impl From<UnixStream> for Oneshot {
    fn from(stream: UnixStream) -> Self {
        stream.into_split().into()
    }
}

/// [Invoke] implementation of a Unix domain socket transport
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

impl<'a> From<&'a std::os::unix::net::SocketAddr> for Client<&'a std::os::unix::net::SocketAddr> {
    fn from(addr: &'a std::os::unix::net::SocketAddr) -> Self {
        Self(addr)
    }
}

impl From<std::os::unix::net::SocketAddr> for Client<std::os::unix::net::SocketAddr> {
    fn from(addr: std::os::unix::net::SocketAddr) -> Self {
        Self(addr)
    }
}

impl Invoke for Client<PathBuf> {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(&self.0).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Invoke for Client<&Path> {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(self.0).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Invoke for Client<&std::os::unix::net::SocketAddr> {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(self.0)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = stream.into_split();
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Invoke for Client<std::os::unix::net::SocketAddr> {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(&self.0)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = stream.into_split();
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Accept for UnixListener {
    type Context = SocketAddr;
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (&self).accept().await
    }
}

impl Accept for &UnixListener {
    type Context = SocketAddr;
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;

    #[instrument(level = "trace")]
    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (stream, addr) = UnixListener::accept(self).await?;
        let (rx, tx) = stream.into_split();
        Ok((addr, tx, rx))
    }
}
