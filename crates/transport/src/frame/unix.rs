//! Unix domain socket transport

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use bytes::Bytes;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf, SocketAddr};
use tokio::net::{UnixListener, UnixStream};
use tracing::instrument;

use crate::frame::{invoke, Accept, Incoming, Outgoing};
use crate::Invoke;

/// [Invoke] implementation in terms of a single [`UnixStream`]
///
/// [`Invoke::invoke`] can only be called once on [Invocation],
/// repeated calls with return an error
pub struct Invocation(std::sync::Mutex<Option<UnixStream>>);

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

impl From<UnixStream> for Invocation {
    fn from(stream: UnixStream) -> Self {
        Self(std::sync::Mutex::new(Some(stream)))
    }
}

impl Invoke for Invocation {
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
        let stream = match self.0.lock() {
            Ok(mut stream) => stream
                .take()
                .context("stream was already used for an invocation")?,
            Err(_) => bail!("stream lock poisoned"),
        };
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
