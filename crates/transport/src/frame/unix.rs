//! Unix domain socket transport

use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tracing::instrument;

use crate::Invoke;
use crate::frame::{Incoming, IncomingLimits, Outgoing, ReadHalf, WriteHalf, invoke};

/// Splits a Unix domain socket stream into a [`ReadHalf`] and a [`WriteHalf`].
///
/// If the [`WriteHalf`] is dropped before being shut down, the socket is closed and reading from
/// the [`ReadHalf`] fails, unless it already reached the end of the stream.
#[must_use]
pub fn split(stream: UnixStream) -> (ReadHalf<UnixStream>, WriteHalf<UnixStream>) {
    crate::frame::split::split(stream, |_| {})
}

/// [Invoke] implementation in terms of a single [`UnixStream`].
///
/// [`Invoke::invoke`] can only be called at most once on [Oneshot],
/// repeated calls will return an error.
pub type Oneshot = super::Oneshot<ReadHalf<UnixStream>, WriteHalf<UnixStream>>;

impl Oneshot {
    /// Creates a pair of connected [Oneshot] using [`UnixStream::pair`].
    pub fn unix_pair() -> std::io::Result<(Oneshot, UnixStream)> {
        let (clt, srv) = UnixStream::pair()?;
        Ok((clt.into(), srv))
    }
}

impl From<UnixStream> for Oneshot {
    fn from(stream: UnixStream) -> Self {
        split(stream).into()
    }
}

/// [Invoke] implementation of a Unix domain socket transport
#[derive(Clone, Debug)]
pub struct Client<T> {
    addr: T,
    limits: IncomingLimits,
}

impl<T> Client<T> {
    fn new(addr: T) -> Self {
        Self {
            addr,
            limits: IncomingLimits::default(),
        }
    }

    /// Bounds the data buffered for the results of invocations by `limits`, which are the
    /// default [`IncomingLimits`] unless set.
    #[must_use]
    pub fn incoming_limits(self, limits: IncomingLimits) -> Self {
        Self { limits, ..self }
    }
}

impl From<PathBuf> for Client<PathBuf> {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

impl<'a> From<&'a Path> for Client<&'a Path> {
    fn from(path: &'a Path) -> Self {
        Self::new(path)
    }
}

impl<'a> From<&'a std::os::unix::net::SocketAddr> for Client<&'a std::os::unix::net::SocketAddr> {
    fn from(addr: &'a std::os::unix::net::SocketAddr) -> Self {
        Self::new(addr)
    }
}

impl From<std::os::unix::net::SocketAddr> for Client<std::os::unix::net::SocketAddr> {
    fn from(addr: std::os::unix::net::SocketAddr) -> Self {
        Self::new(addr)
    }
}

impl Invoke for Client<PathBuf> {
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(&self.addr).await?;
        let (rx, tx) = split(stream);
        invoke(
            tx,
            BufReader::new(rx),
            instance,
            func,
            params,
            paths,
            self.limits,
        )
        .await
    }
}

impl Invoke for Client<&Path> {
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = UnixStream::connect(self.addr).await?;
        let (rx, tx) = split(stream);
        invoke(
            tx,
            BufReader::new(rx),
            instance,
            func,
            params,
            paths,
            self.limits,
        )
        .await
    }
}

impl Invoke for Client<&std::os::unix::net::SocketAddr> {
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(self.addr)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = split(stream);
        invoke(
            tx,
            BufReader::new(rx),
            instance,
            func,
            params,
            paths,
            self.limits,
        )
        .await
    }
}

impl Invoke for Client<std::os::unix::net::SocketAddr> {
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = std::os::unix::net::UnixStream::connect_addr(&self.addr)?;
        let stream = UnixStream::from_std(stream)?;
        let (rx, tx) = split(stream);
        invoke(
            tx,
            BufReader::new(rx),
            instance,
            func,
            params,
            paths,
            self.limits,
        )
        .await
    }
}
