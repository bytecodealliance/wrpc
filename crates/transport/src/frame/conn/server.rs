use core::fmt::{Debug, Display};
use core::marker::PhantomData;

use std::collections::{HashMap, hash_map};
use std::sync::Arc;

use anyhow::bail;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{instrument, trace};

use crate::Serve;
use crate::frame::{ConnHandler, Header, HeaderReadError, Incoming, Outgoing};

/// wRPC server for framed transports
#[derive(Debug)]
pub struct Server<C, I, O, H = ()> {
    handlers: Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, I, O)>>>>,
    conn_handler: PhantomData<H>,
}

impl<C, I, O, H> Server<C, I, O, H> {
    /// Constructs a new [Server]
    #[must_use]
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
    /// Header read error
    HeaderRead(HeaderReadError),
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

impl<C, I, O> From<HeaderReadError> for AcceptError<C, I, O> {
    fn from(err: HeaderReadError) -> Self {
        Self::HeaderRead(err)
    }
}

impl<C, I, O> From<mpsc::error::SendError<(C, I, O)>> for AcceptError<C, I, O> {
    fn from(err: mpsc::error::SendError<(C, I, O)>) -> Self {
        Self::Send(err)
    }
}

impl<C, I, O> Debug for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeaderRead(err) => Debug::fmt(err, f),
            Self::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            Self::Send(err) => Debug::fmt(err, f),
        }
    }
}

impl<C, I, O> Display for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeaderRead(err) => Display::fmt(err, f),
            Self::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            Self::Send(err) => Display::fmt(err, f),
        }
    }
}

impl<C, I, O> core::error::Error for AcceptError<C, I, O> {}

impl<C, I, O, H> Server<C, I, O, H>
where
    I: AsyncRead + Unpin,
    H: ConnHandler<I, O>,
{
    /// Accept an already-established connection.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the invocation fails
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn accept(&self, cx: C, tx: O, mut rx: I) -> Result<(), AcceptError<C, I, O>> {
        let Header { instance, name } = Header::read(&mut rx).await?;
        let h = self.handlers.lock().await;
        let h = h
            .get(&instance)
            .and_then(|h| h.get(&name))
            .ok_or_else(|| AcceptError::UnhandledFunction { instance, name })?;
        h.send((cx, rx, tx)).await?;
        Ok(())
    }
}

#[instrument(level = "trace", skip(srv, paths))]
async fn serve<C, I, O, H>(
    srv: &Server<C, I, O, H>,
    instance: &str,
    func: &str,
    paths: Arc<[Box<[Option<usize>]>]>,
) -> anyhow::Result<
    impl Stream<Item = anyhow::Result<(C, Outgoing, Incoming)>> + 'static + use<C, I, O, H>,
>
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
    Ok(ReceiverStream::new(rx).map(move |(cx, rx, tx)| {
        trace!("received invocation");
        let rx = Incoming::new(rx, paths.as_ref(), |rx, res| H::on_ingress(rx, res));
        let tx = Outgoing::new(tx, |tx, res| H::on_egress(tx, res));
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

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Outgoing, Incoming)>>
        + 'static
        + use<C, I, O, H>,
    > {
        serve(self, instance, func, paths).await
    }
}

impl<'a, C, I, O, H> Serve for &'a Server<C, I, O, H>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    H: ConnHandler<I, O> + Send + Sync,
{
    type Context = C;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Outgoing, Incoming)>>
        + 'static
        + use<'a, C, I, O, H>,
    > {
        serve(self, instance, func, paths).await
    }
}
