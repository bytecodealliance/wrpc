use core::fmt::{Debug, Display};
use core::future::poll_fn;

use std::collections::{HashMap, hash_map};
use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{instrument, trace};

use crate::Serve;
use crate::frame::conn::poll_decode;
use crate::frame::{Header, HeaderDecoder, HeaderReadError};

/// wRPC server for framed transports, which receives invocations over streams of type `I` and
/// transmits over streams of type `O`
#[derive(Debug)]
pub struct Server<C, I, O> {
    handlers: Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, O, I)>>>>,
    header: HeaderDecoder,
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
pub enum AcceptError<C, I, O> {
    /// I/O error
    IO(std::io::Error),
    /// Header read error
    HeaderRead(HeaderReadError),
    /// Function was not handled
    UnhandledFunction {
        /// Instance
        instance: String,
        /// Function name
        name: String,
    },
    /// The handler of the function stopped receiving invocations
    Send(SendError<(C, O, I)>),
}

impl<C, I, O> From<HeaderReadError> for AcceptError<C, I, O> {
    fn from(err: HeaderReadError) -> Self {
        Self::HeaderRead(err)
    }
}

impl<C, I, O> Debug for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Debug::fmt(err, f),
            Self::HeaderRead(err) => Debug::fmt(err, f),
            Self::UnhandledFunction { .. } => Display::fmt(self, f),
            Self::Send(err) => Debug::fmt(err, f),
        }
    }
}

impl<C, I, O> Display for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Display::fmt(err, f),
            Self::HeaderRead(err) => Display::fmt(err, f),
            Self::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            Self::Send(err) => Display::fmt(err, f),
        }
    }
}

impl<C, I, O> core::error::Error for AcceptError<C, I, O> {}

impl<C, I, O> Server<C, I, O>
where
    I: AsyncBufRead + Unpin,
{
    /// Accept an invocation on an already-established connection, passing its streams to the
    /// handler of the function it invokes.
    ///
    /// The streams are dropped if the invocation is not accepted.
    #[instrument(level = "trace", skip_all)]
    pub async fn accept(&self, cx: C, tx: O, mut rx: I) -> Result<(), AcceptError<C, I, O>> {
        let mut dec = self.header.clone();
        let max = dec.max_len();
        let mut buf = BytesMut::default();
        let header = poll_fn(|cx| poll_decode(&mut rx, &mut dec, max, &mut buf, cx)).await?;
        let Some(Header { instance, name }) = header else {
            return Err(HeaderReadError::IO(std::io::ErrorKind::UnexpectedEof.into()).into());
        };
        let h = self
            .handlers
            .lock()
            .await
            .get(&instance)
            .and_then(|h| h.get(&name))
            .cloned()
            .ok_or(AcceptError::UnhandledFunction { instance, name })?;
        h.send((cx, tx, rx)).await.map_err(AcceptError::Send)
    }
}

#[instrument(level = "trace", skip(srv))]
async fn serve<C, I, O>(
    srv: &Server<C, I, O>,
    instance: &str,
    func: &str,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<(C, O, I)>> + 'static + use<C, I, O>>
where
    C: Send + Sync + 'static,
    I: Send + 'static,
    O: Send + 'static,
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
    Ok(ReceiverStream::new(rx).map(|invocation| {
        trace!("received invocation");
        Ok(invocation)
    }))
}

impl<C, I, O> Serve for Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    type Context = C;
    type Outgoing = O;
    type Incoming = I;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        _paths: Arc<[Box<[Option<usize>]>]>,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
        + 'static
        + use<C, I, O>,
    > {
        serve(self, instance, func).await
    }
}

impl<'a, C, I, O> Serve for &'a Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    type Context = C;
    type Outgoing = O;
    type Incoming = I;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        _paths: Arc<[Box<[Option<usize>]>]>,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
        + 'static
        + use<'a, C, I, O>,
    > {
        serve(self, instance, func).await
    }
}
