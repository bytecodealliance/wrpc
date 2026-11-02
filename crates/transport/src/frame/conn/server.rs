use core::fmt::{Debug, Display};

use std::collections::{HashMap, hash_map};
use std::sync::Arc;

use anyhow::bail;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{instrument, trace};

use crate::Serve;
use crate::frame::{Header, HeaderDecoder, HeaderReadError, Incoming, Outgoing, Reject};

/// wRPC server for framed transports
#[derive(Debug)]
pub struct Server<C, I, O> {
    handlers: Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, Unhandled<I>, O)>>>>,
    header: HeaderDecoder,
}

/// Reader of an invocation, which is rejected once dropped before being handed to a handler
#[derive(Debug)]
struct Unhandled<I> {
    rx: Option<I>,
    reject: fn(I),
}

impl<I> Unhandled<I> {
    fn get_mut(&mut self) -> &mut I {
        self.rx.as_mut().expect("reader is only taken by handlers")
    }

    fn handle(mut self) -> I {
        self.rx.take().expect("reader is only taken by handlers")
    }
}

impl<I> Drop for Unhandled<I> {
    fn drop(&mut self) {
        if let Some(rx) = self.rx.take() {
            (self.reject)(rx);
        }
    }
}

impl<C, I, O> Server<C, I, O> {
    /// Constructs a new [Server]
    #[must_use]
    pub fn new() -> Self {
        Self::with_header_decoder(HeaderDecoder::default())
    }

    /// Constructs a new [Server], which reads invocation headers using `header`
    #[must_use]
    pub fn with_header_decoder(header: HeaderDecoder) -> Self {
        Self {
            handlers: Mutex::default(),
            header,
        }
    }
}

impl<C, I, O> Default for Server<C, I, O> {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned by [`Server::accept`]
pub enum AcceptError {
    /// Header read error
    HeaderRead(HeaderReadError),
    /// Function was not handled
    UnhandledFunction {
        /// Instance
        instance: String,
        /// Function name
        name: String,
    },
}

impl From<HeaderReadError> for AcceptError {
    fn from(err: HeaderReadError) -> Self {
        Self::HeaderRead(err)
    }
}

impl Debug for AcceptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HeaderRead(err) => Debug::fmt(err, f),
            Self::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
        }
    }
}

impl Display for AcceptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HeaderRead(err) => Display::fmt(err, f),
            Self::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
        }
    }
}

impl core::error::Error for AcceptError {}

impl<C, I, O> Server<C, I, O>
where
    I: AsyncBufRead + Reject + Unpin,
{
    /// Accept an already-established connection.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the invocation fails. `rx` is rejected if the invocation is
    /// not handed to a handler, i.e. if reading the invocation header fails, the function is not
    /// handled, its handler stops receiving invocations or the returned future is dropped.
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn accept(&self, cx: C, tx: O, rx: I) -> Result<(), AcceptError> {
        let mut rx = Unhandled {
            rx: Some(rx),
            reject: I::reject,
        };
        let Header { instance, name } = self.header.read(rx.get_mut()).await?;
        let h = self
            .handlers
            .lock()
            .await
            .get(&instance)
            .and_then(|h| h.get(&name))
            .cloned();
        let unhandled = || AcceptError::UnhandledFunction { instance, name };
        let Some(h) = h else {
            return Err(unhandled());
        };
        h.send((cx, rx, tx)).await.map_err(|_| unhandled())
    }
}

#[instrument(level = "trace", skip(srv, paths))]
async fn serve<C, I, O>(
    srv: &Server<C, I, O>,
    instance: &str,
    func: &str,
    paths: Arc<[Box<[Option<usize>]>]>,
) -> anyhow::Result<
    impl Stream<Item = anyhow::Result<(C, Outgoing, Incoming)>> + 'static + use<C, I, O>,
>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Reject + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
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
        let rx = Incoming::new(rx.handle(), paths.as_ref());
        let tx = Outgoing::new(tx);
        Ok((cx, tx, rx))
    }))
}

impl<C, I, O> Serve for Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Reject + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Context = C;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Outgoing, Incoming)>> + 'static + use<C, I, O>,
    > {
        serve(self, instance, func, paths).await
    }
}

impl<'a, C, I, O> Serve for &'a Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Reject + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
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
        + use<'a, C, I, O>,
    > {
        serve(self, instance, func, paths).await
    }
}
