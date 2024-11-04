use core::fmt::{Debug, Display};
use core::marker::PhantomData;

use std::collections::{hash_map, HashMap};
use std::sync::Arc;

use anyhow::bail;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{instrument, trace};
use wasm_tokio::AsyncReadCore as _;

use crate::frame::conn::Accept;
use crate::frame::{Conn, ConnHandler, Incoming, Outgoing};
use crate::Serve;

/// wRPC server for framed transports
pub struct Server<C, I, O, H = ()> {
    handlers: Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, I, O)>>>>,
    conn_handler: PhantomData<H>,
}

impl<C, I, O, H> Server<C, I, O, H> {
    /// Constructs a new [Server]
    pub fn new() -> Self {
        Self {
            handlers: Mutex::default(),
            conn_handler: PhantomData,
        }
    }
}

impl<C, I, O> Default for Server<C, I, O> {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned by [`Server::accept`]
pub enum AcceptError<C, I, O> {
    /// I/O error
    IO(std::io::Error),
    /// Protocol version is not supported
    UnsupportedVersion(u8),
    /// Function was not handled
    UnhandledFunction {
        /// Instance
        instance: String,
        /// Function name
        name: String,
    },
    /// Message sending failed
    Send(mpsc::error::SendError<(C, I, O)>),
}

impl<C, I, O> Debug for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::IO(err) => Debug::fmt(err, f),
            AcceptError::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
            AcceptError::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            AcceptError::Send(err) => Debug::fmt(err, f),
        }
    }
}

impl<C, I, O> Display for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::IO(err) => Display::fmt(err, f),
            AcceptError::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
            AcceptError::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            AcceptError::Send(err) => Display::fmt(err, f),
        }
    }
}

impl<C, I, O> std::error::Error for AcceptError<C, I, O> {}

impl<C, I, O, H> Server<C, I, O, H>
where
    I: AsyncRead + Unpin,
    H: ConnHandler<I, O>,
{
    /// Accept a connection on an [Accept].
    ///
    /// # Errors
    ///
    /// Returns an error if accepting the connection has failed
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn accept(
        &self,
        listener: impl Accept<Context = C, Incoming = I, Outgoing = O>,
    ) -> Result<(), AcceptError<C, I, O>> {
        let (cx, tx, mut rx) = listener.accept().await.map_err(AcceptError::IO)?;
        let mut instance = String::default();
        let mut name = String::default();
        match rx.read_u8().await.map_err(AcceptError::IO)? {
            0x00 => {
                rx.read_core_name(&mut instance)
                    .await
                    .map_err(AcceptError::IO)?;
                rx.read_core_name(&mut name)
                    .await
                    .map_err(AcceptError::IO)?;
            }
            v => return Err(AcceptError::UnsupportedVersion(v)),
        }
        let h = self.handlers.lock().await;
        let h = h
            .get(&instance)
            .and_then(|h| h.get(&name))
            .ok_or_else(|| AcceptError::UnhandledFunction { instance, name })?;
        h.send((cx, rx, tx)).await.map_err(AcceptError::Send)?;
        Ok(())
    }
}

#[instrument(level = "trace", skip(srv, paths))]
async fn serve<C, I, O, H>(
    srv: &Server<C, I, O, H>,
    instance: &str,
    func: &str,
    paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<(C, Outgoing, Incoming)>> + 'static>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    H: ConnHandler<I, O>,
{
    let (tx, rx) = mpsc::channel(1024);
    let mut handlers = srv.handlers.lock().await;
    match handlers
        .entry(instance.to_string())
        .or_default()
        .entry(func.to_string())
    {
        hash_map::Entry::Occupied(_) => {
            bail!("handler for `{instance}#{func}` already exists")
        }
        hash_map::Entry::Vacant(entry) => {
            entry.insert(tx);
        }
    }
    let paths = paths.into();
    Ok(ReceiverStream::new(rx).map(move |(cx, rx, tx)| {
        trace!("received invocation");
        let Conn { tx, rx } = Conn::new::<H, _, _, _>(rx, tx, paths.iter());
        Ok((cx, tx, rx))
    }))
}

impl<C, I, O, H> Serve for Server<C, I, O, H>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    H: ConnHandler<I, O> + Send + Sync,
{
    type Context = C;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>> + 'static,
    > {
        serve(self, instance, func, paths).await
    }
}

impl<C, I, O, H> Serve for &Server<C, I, O, H>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    H: ConnHandler<I, O> + Send + Sync,
{
    type Context = C;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>> + 'static,
    > {
        serve(self, instance, func, paths).await
    }
}
