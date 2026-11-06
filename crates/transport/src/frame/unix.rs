//! Unix domain socket transport

use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::io::BufReader;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf, SocketAddr};
use tokio::net::{UnixListener, UnixStream};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{AcceptError, invoke};

/// Unix domain socket server
pub type Server = crate::frame::Server<SocketAddr, BufReader<OwnedReadHalf>, OwnedWriteHalf>;

/// Accepts a connection on `lis` and an invocation on it with the peer address as the context,
/// see [`Server::accept`].
///
/// # Errors
///
/// Returns an error if accepting the connection or the invocation fails
pub async fn accept(
    srv: &Server,
    lis: &UnixListener,
) -> Result<(), AcceptError<SocketAddr, BufReader<OwnedReadHalf>, OwnedWriteHalf>> {
    let (stream, addr) = lis.accept().await.map_err(AcceptError::IO)?;
    let (rx, tx) = stream.into_split();
    srv.accept(addr, tx, BufReader::new(rx)).await
}

/// [Invoke] implementation in terms of a single [`UnixStream`].
///
/// [`Invoke::invoke`] can only be called at most once on [Oneshot],
/// repeated calls will return an error.
pub type Oneshot = super::Oneshot<OwnedReadHalf, OwnedWriteHalf>;

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
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;

    #[instrument(level = "trace", skip(self, _paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(&self.0).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, BufReader::new(rx), instance, func, params).await
    }
}

impl Invoke for Client<&Path> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;

    #[instrument(level = "trace", skip(self, _paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(self.0).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, BufReader::new(rx), instance, func, params).await
    }
}

impl Invoke for Client<&std::os::unix::net::SocketAddr> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;

    #[instrument(level = "trace", skip(self, _paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(self.0)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = stream.into_split();
        invoke(tx, BufReader::new(rx), instance, func, params).await
    }
}

impl Invoke for Client<std::os::unix::net::SocketAddr> {
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;

    #[instrument(level = "trace", skip(self, _paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(&self.0)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = stream.into_split();
        invoke(tx, BufReader::new(rx), instance, func, params).await
    }
}
