//! TCP transport

use core::net::SocketAddr;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tracing::instrument;

use crate::frame::{invoke, Accept, Incoming, Outgoing};
use crate::Invoke;

/// [Invoke] implementation in terms of a single [TcpStream]
///
/// [`Invoke::invoke`] can only be called once on [Invocation],
/// repeated calls with return an error
pub struct Invocation(std::sync::Mutex<Option<TcpStream>>);

/// [Invoke] implementation of a TCP transport
#[derive(Clone, Debug)]
pub struct Client<T>(T);

impl<T> From<T> for Client<T>
where
    T: ToSocketAddrs + Clone,
{
    fn from(path: T) -> Self {
        Self(path)
    }
}

impl From<TcpStream> for Invocation {
    fn from(stream: TcpStream) -> Self {
        Self(std::sync::Mutex::new(Some(stream)))
    }
}

impl<T> Invoke for Client<T>
where
    T: ToSocketAddrs + Clone + Send + Sync,
{
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
        let stream = TcpStream::connect(self.0.clone()).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, rx, instance, func, params, paths).await
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

impl Accept for TcpListener {
    type Context = SocketAddr;
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (&self).accept().await
    }
}

impl Accept for &TcpListener {
    type Context = SocketAddr;
    type Outgoing = OwnedWriteHalf;
    type Incoming = OwnedReadHalf;

    #[instrument(level = "trace")]
    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (stream, addr) = TcpListener::accept(self).await?;
        let (rx, tx) = stream.into_split();
        Ok((addr, tx, rx))
    }
}
