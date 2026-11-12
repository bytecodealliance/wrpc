use core::fmt::{Debug, Display};

use std::collections::{HashMap, hash_map};
use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};

use crate::Serve;
use crate::frame::{
    DEFAULT_MAX_NAME_LEN, Header, HeaderDecoder, HeaderReadError, Incoming, Outgoing,
};

/// wRPC server for framed transports
#[derive(Debug)]
pub struct Server<C, I, O> {
    handlers: Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, I, O)>>>>,
    max_instance_len: u32,
    max_name_len: u32,
}

impl<C, I, O> Server<C, I, O> {
    /// Constructs a new [Server]
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_name_len(DEFAULT_MAX_NAME_LEN, DEFAULT_MAX_NAME_LEN)
    }

    /// Constructs a new [Server], which rejects invocations of instance and function names longer
    /// than `max_instance_len` and `max_name_len` bytes respectively
    #[must_use]
    pub fn with_max_name_len(max_instance_len: u32, max_name_len: u32) -> Self {
        Self {
            handlers: Mutex::default(),
            max_instance_len,
            max_name_len,
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
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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

impl<C, I, O> Server<C, I, O>
where
    I: AsyncBufRead + Unpin,
{
    /// Accept an already-established connection.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the invocation fails
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn accept(&self, cx: C, tx: O, mut rx: I) -> Result<(), AcceptError<C, I, O>> {
        let mut dec = HeaderDecoder::new(self.max_instance_len, self.max_name_len);
        let mut buf = BytesMut::default();
        let Header { instance, name } = loop {
            let chunk = rx.fill_buf().await.map_err(HeaderReadError::IO)?;
            if chunk.is_empty() {
                return Err(AcceptError::HeaderRead(HeaderReadError::IO(
                    std::io::ErrorKind::UnexpectedEof.into(),
                )));
            }
            let n = chunk.len();
            buf.extend_from_slice(chunk);
            if let Some(header) = dec.decode(&mut buf)? {
                rx.consume(n.saturating_sub(buf.len()));
                break header;
            }
            rx.consume(n);
        };
        let h = self
            .handlers
            .lock()
            .await
            .get(&instance)
            .and_then(|h| h.get(&name))
            .cloned()
            .ok_or_else(|| AcceptError::UnhandledFunction { instance, name })?;
        h.send((cx, rx, tx)).await?;
        Ok(())
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
    I: AsyncBufRead + Send + Sync + Unpin + 'static,
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
        let rx = Incoming::new(rx, paths.as_ref());
        let tx = Outgoing::new(tx);
        Ok((cx, tx, rx))
    }))
}

impl<C, I, O> Serve for Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncBufRead + Send + Sync + Unpin + 'static,
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
    I: AsyncBufRead + Send + Sync + Unpin + 'static,
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
