use core::fmt::{Debug, Display};
use core::future::poll_fn;
use core::mem;
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker, ready};
use core::time::Duration;

use std::sync::{Arc, PoisonError};

use bytes::{Buf as _, Bytes, BytesMut};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt as _};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Sleep;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tokio_util::task::AbortOnDropHandle;
use tracing::{Instrument as _, Span, debug, error, instrument, trace};
use wasm_tokio::{
    AsyncReadCore as _, AsyncReadLeb128 as _, DEFAULT_MAX_INITIAL_CAPACITY, Leb128Encoder,
};

use crate::frame::MAX_INITIAL_PATH_CAPACITY;
use crate::{copy_io_error, error_receiver_closed};

mod client;
mod server;

pub use client::*;
pub use server::*;

/// Error returned by [`Header::read`]
pub enum HeaderReadError {
    /// I/O error
    IO(std::io::Error),
    /// Protocol version is not supported
    UnsupportedVersion(u8),
}

impl Debug for HeaderReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Debug::fmt(err, f),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
        }
    }
}

impl Display for HeaderReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Display::fmt(err, f),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
        }
    }
}

impl core::error::Error for HeaderReadError {}

/// wRPC invocation header
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Header {
    /// Instance name of the function being called
    pub instance: String,

    /// Name of the function being called
    pub name: String,
}

impl Header {
    /// Reads the wRPC header from a byte stream
    ///
    /// # Errors
    ///
    /// Returns an error if reading the header fails
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn read(mut rx: impl AsyncRead + Unpin) -> Result<Self, HeaderReadError> {
        let mut instance = String::default();
        let mut name = String::default();
        match rx.read_u8().await.map_err(HeaderReadError::IO)? {
            0x00 => {
                rx.read_core_name(&mut instance)
                    .await
                    .map_err(HeaderReadError::IO)?;
                rx.read_core_name(&mut name)
                    .await
                    .map_err(HeaderReadError::IO)?;
            }
            v => return Err(HeaderReadError::UnsupportedVersion(v)),
        }
        Ok(Self { instance, name })
    }
}

/// Index trie containing async stream subscriptions
#[derive(Debug, Default)]
enum IndexTrie {
    #[default]
    Empty,
    Leaf {
        tx: Option<mpsc::Sender<std::io::Result<Bytes>>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    },
    IndexNode {
        tx: Option<mpsc::Sender<std::io::Result<Bytes>>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        nested: Vec<Option<IndexTrie>>,
    },
    // TODO: Add partially-indexed `WildcardIndexNode`
    WildcardNode {
        tx: Option<mpsc::Sender<std::io::Result<Bytes>>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        nested: Option<Box<IndexTrie>>,
    },
}

impl<'a>
    From<(
        &'a [Option<usize>],
        mpsc::Sender<std::io::Result<Bytes>>,
        Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    )> for IndexTrie
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            mpsc::Sender<std::io::Result<Bytes>>,
            Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        ),
    ) -> Self {
        match path {
            [] => Self::Leaf { tx: Some(tx), rx },
            [None, path @ ..] => Self::WildcardNode {
                tx: None,
                rx: None,
                nested: Some(Box::new(Self::from((path, tx, rx)))),
            },
            [Some(i), path @ ..] => Self::IndexNode {
                tx: None,
                rx: None,
                nested: {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, tx, rx)));
                    nested
                },
            },
        }
    }
}

impl<'a>
    From<(
        &'a [Option<usize>],
        mpsc::Sender<std::io::Result<Bytes>>,
        mpsc::Receiver<std::io::Result<Bytes>>,
    )> for IndexTrie
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            mpsc::Sender<std::io::Result<Bytes>>,
            mpsc::Receiver<std::io::Result<Bytes>>,
        ),
    ) -> Self {
        Self::from((path, tx, Some(rx)))
    }
}

impl<'a> From<(&'a [Option<usize>], mpsc::Sender<std::io::Result<Bytes>>)> for IndexTrie {
    fn from((path, tx): (&'a [Option<usize>], mpsc::Sender<std::io::Result<Bytes>>)) -> Self {
        Self::from((path, tx, None))
    }
}

impl<P: AsRef<[Option<usize>]>> FromIterator<P> for IndexTrie {
    fn from_iter<T: IntoIterator<Item = P>>(iter: T) -> Self {
        let mut root = Self::Empty;
        for path in iter {
            let (tx, rx) = mpsc::channel(16);
            if !root.insert(path.as_ref(), tx, Some(rx)) {
                return Self::Empty;
            }
        }
        root
    }
}

impl IndexTrie {
    /// Calls `f` with the sender and receiver of each node in the trie
    fn for_each(
        &mut self,
        f: &mut impl FnMut(
            &mut Option<mpsc::Sender<std::io::Result<Bytes>>>,
            &mut Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        ),
    ) {
        match self {
            Self::Empty => {}
            Self::Leaf { tx, rx } => f(tx, rx),
            Self::IndexNode { tx, rx, nested } => {
                f(tx, rx);
                for nested in nested.iter_mut().flatten() {
                    nested.for_each(f);
                }
            }
            Self::WildcardNode { tx, rx, nested } => {
                f(tx, rx);
                if let Some(nested) = nested {
                    nested.for_each(f);
                }
            }
        }
    }

    /// Closes all receivers in the trie, returning `true` if any of them held data
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn close_rx(&mut self) -> bool {
        let mut pending = false;
        self.for_each(&mut |_, rx| {
            pending |= rx.take().is_some_and(|rx| !rx.is_empty());
        });
        pending
    }

    /// Takes the receiver
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn take_rx(&mut self, path: &[usize]) -> Option<mpsc::Receiver<std::io::Result<Bytes>>> {
        let Some((i, path)) = path.split_first() else {
            return match self {
                Self::Empty => None,
                Self::Leaf { rx, .. } => rx.take(),
                Self::IndexNode { tx, rx, nested } => {
                    let rx = rx.take();
                    if nested.is_empty() && tx.is_none() {
                        *self = Self::Empty;
                    }
                    rx
                }
                Self::WildcardNode { tx, rx, nested } => {
                    let rx = rx.take();
                    if nested.is_none() && tx.is_none() {
                        *self = Self::Empty;
                    }
                    rx
                }
            };
        };
        match self {
            Self::Empty | Self::Leaf { .. } | Self::WildcardNode { .. } => None,
            Self::IndexNode { nested, .. } => nested
                .get_mut(*i)
                .and_then(|nested| nested.as_mut().and_then(|nested| nested.take_rx(path))),
            // TODO: Demux the subscription
            //Self::WildcardNode { ref mut nested, .. } => {
            //    nested.as_mut().and_then(|nested| nested.take(path))
            //}
        }
    }

    /// Gets a sender
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn get_tx(&mut self, path: &[usize]) -> Option<mpsc::Sender<std::io::Result<Bytes>>> {
        let Some((i, path)) = path.split_first() else {
            return match self {
                Self::Empty => None,
                Self::Leaf { tx, .. } => tx.clone(),
                Self::IndexNode { tx, .. } | Self::WildcardNode { tx, .. } => tx.clone(),
            };
        };
        match self {
            Self::Empty | Self::Leaf { .. } | Self::WildcardNode { .. } => None,
            Self::IndexNode { nested, .. } => {
                let nested = nested.get_mut(*i)?;
                let nested = nested.as_mut()?;
                nested.get_tx(path)
            } // TODO: Demux the subscription
              //Self::WildcardNode { ref mut nested, .. } => {
              //    nested.as_mut().and_then(|nested| nested.take(path))
              //}
        }
    }

    /// Closes all senders in the trie
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn close_tx(&mut self) {
        self.for_each(&mut |tx, _| {
            tx.take();
        });
    }

    /// Inserts `sender` and `receiver` under a `path` - returns `false` if it failed and `true` if it succeeded.
    /// Tree state after `false` is returned is undefined
    #[instrument(level = "trace", skip(self, sender, receiver), ret(level = "trace"))]
    fn insert(
        &mut self,
        path: &[Option<usize>],
        sender: mpsc::Sender<std::io::Result<Bytes>>,
        receiver: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    ) -> bool {
        match self {
            Self::Empty => {
                *self = Self::from((path, sender, receiver));
                true
            }
            Self::Leaf { .. } => {
                let Some((i, path)) = path.split_first() else {
                    return false;
                };
                let Self::Leaf { tx, rx } = mem::take(self) else {
                    return false;
                };
                if let Some(i) = i {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, sender, receiver)));
                    *self = Self::IndexNode { tx, rx, nested };
                } else {
                    *self = Self::WildcardNode {
                        tx,
                        rx,
                        nested: Some(Box::new(Self::from((path, sender, receiver)))),
                    };
                }
                true
            }
            Self::IndexNode { tx, rx, nested } => match (&tx, &rx, path) {
                (None, None, []) => {
                    *tx = Some(sender);
                    *rx = receiver;
                    true
                }
                (_, _, [Some(i), path @ ..]) => {
                    let cap = i.saturating_add(1);
                    if nested.len() < cap {
                        nested.resize_with(cap, Option::default);
                    }
                    let nested = &mut nested[*i];
                    if let Some(nested) = nested {
                        nested.insert(path, sender, receiver)
                    } else {
                        *nested = Some(Self::from((path, sender, receiver)));
                        true
                    }
                }
                _ => false,
            },
            Self::WildcardNode { tx, rx, nested } => match (&tx, &rx, path) {
                (None, None, []) => {
                    *tx = Some(sender);
                    *rx = receiver;
                    true
                }
                (_, _, [None, path @ ..]) => {
                    if let Some(nested) = nested {
                        nested.insert(path, sender, receiver)
                    } else {
                        *nested = Some(Box::new(Self::from((path, sender, receiver))));
                        true
                    }
                }
                _ => false,
            },
        }
    }
}

pin_project! {
    /// Incoming framed stream
    #[project = IncomingProj]
    pub struct Incoming {
        #[pin]
        rx: Option<StreamReader<ReceiverStream<std::io::Result<Bytes>>, Bytes>>,
        path: Arc<[usize]>,
        io: Arc<IngressGuard>,
        state: Arc<IngressState>,
        deadline: Option<Pin<Box<Sleep>>>,
    }

    impl PinnedDrop for Incoming {
        fn drop(this: Pin<&mut Self>) {
            let this = Pin::into_inner(this);
            if let Some(rx) = this.rx.take() {
                let (mut rx, chunk) = rx.into_inner_with_chunk();
                rx.close();
                if chunk.is_some() || !rx.as_ref().is_empty() {
                    this.mark_unprocessed();
                }
            }
        }
    }
}

impl Incoming {
    /// Creates a new [Incoming] given an [`AsyncBufRead`], [`ConnHandler`] and a set of async paths.
    /// `on_ingress` will be called once data ingress is complete.
    /// If `drain` is `true`, data ingress continues after all handles are dropped until the peer
    /// closes the stream, failing on any data received or not processed, otherwise data ingress
    /// is aborted, see [`ConnHandler::DRAIN`].
    pub fn new<T, P, Fut>(
        mut rx: T,
        paths: impl IntoIterator<Item = P>,
        drain: bool,
        on_ingress: impl FnOnce(T, std::io::Result<()>) -> Fut + Send + 'static,
    ) -> Self
    where
        T: AsyncBufRead + Unpin + Send + 'static,
        P: AsRef<[Option<usize>]>,
        Fut: Future<Output = ()> + Send,
    {
        let (rx_tx, rx_rx) = mpsc::channel(128);
        let state = Arc::new(IngressState {
            index: std::sync::Mutex::new(paths.into_iter().collect()),
            err: std::sync::OnceLock::default(),
            unprocessed: AtomicBool::default(),
            timeout: std::sync::Mutex::default(),
        });
        let (dropped_tx, dropped_rx) = drain.then(oneshot::channel).unzip();
        let span = Span::current();
        let task = tokio::spawn({
            let state = Arc::clone(&state);
            async move {
                let res = async {
                    let mut io = pin!(async {
                        let res = ingress(&mut rx, &state.index, &rx_tx).await;
                        if let Err(err) = &res {
                            _ = state.err.set(copy_io_error(err));
                        }
                        drop(rx_tx);
                        trace!("shutting down index trie");
                        state
                            .index
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .close_tx();
                        res
                    });
                    let Some(mut dropped) = dropped_rx else {
                        return io.await;
                    };
                    select! {
                        biased;

                        res = &mut io => {
                            res?;
                            _ = dropped.await;
                            close_rx(&state)
                        }
                        _ = &mut dropped => {
                            close_rx(&state)?;
                            let timeout = *state
                                .timeout
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            match timeout {
                                Some(timeout) => tokio::time::timeout(timeout, io)
                                    .await
                                    .map_err(|_| {
                                        std::io::Error::new(
                                            std::io::ErrorKind::TimedOut,
                                            "drain timed out",
                                        )
                                    })?,
                                None => io.await,
                            }
                        }
                    }
                }
                .await;
                on_ingress(rx, res).await;
            }
            .instrument(span.clone())
        });
        Self {
            rx: Some(StreamReader::new(ReceiverStream::new(rx_rx))),
            path: Arc::from([]),
            io: Arc::new(match dropped_tx {
                Some(dropped) => IngressGuard::Drain { _dropped: dropped },
                None => IngressGuard::Abort {
                    _task: AbortOnDropHandle::new(task),
                },
            }),
            state,
            deadline: None,
        }
    }

    /// Sets the read timeout.
    ///
    /// Once set, a read that stays pending for longer than `timeout` fails with
    /// [`std::io::ErrorKind::TimedOut`]. The deadline is armed when a read first returns
    /// [`Poll::Pending`] and cleared once a read completes or times out. Dropping a pending
    /// read does not clear it, so the next read fails as soon as the deadline elapses unless
    /// data arrives first. The timeout is shared by all sub-streams of this connection,
    /// including those returned by [`Self::index`] and backing async `stream` and `future`
    /// values, which must therefore not idle for longer than `timeout`.
    ///
    /// If ingress is drained, see [`ConnHandler::DRAIN`], the peer must also close the stream
    /// within `timeout` once all handles are dropped, otherwise ingress fails with
    /// [`std::io::ErrorKind::TimedOut`].
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        *self
            .state
            .timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(timeout);
        self
    }

    /// Marks data received by this handle as not processed, which fails ingress once all
    /// handles are dropped
    pub(crate) fn mark_unprocessed(&self) {
        trace!(path = ?self.path, "dropping handle with unprocessed data");
        self.state.unprocessed.store(true, Ordering::Relaxed);
    }

    /// Index the incoming stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it.
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        let path = if self.path.is_empty() {
            Arc::from(path)
        } else {
            Arc::from([self.path.as_ref(), path].concat())
        };
        trace!("locking index trie");
        let mut index = self
            .state
            .index
            .lock()
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        trace!(?path, "taking index subscription");
        let rx = index
            .take_rx(&path)
            .map(|rx| StreamReader::new(ReceiverStream::new(rx)));
        Ok(Self {
            rx,
            path,
            io: Arc::clone(&self.io),
            state: Arc::clone(&self.state),
            deadline: None,
        })
    }
}

impl AsyncRead for Incoming {
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        trace!("reading");
        let this = self.as_mut().project();
        let Some(rx) = this.rx.as_pin_mut() else {
            trace!("reader is closed");
            return Poll::Ready(Ok(()));
        };
        if let Poll::Ready(res) = rx.poll_read(cx, buf) {
            *this.deadline = None;
            res?;
        } else {
            let timeout = *this
                .state
                .timeout
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(timeout) = timeout else {
                return Poll::Pending;
            };
            let sleep = this
                .deadline
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
            ready!(sleep.as_mut().poll(cx));
            *this.deadline = None;
            trace!("read timed out");
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "read timed out",
            )));
        }
        trace!(buf = ?buf.filled(), "read buffer");
        if buf.filled().is_empty() {
            self.rx.take();
            if let Some(err) = self.state.err.get() {
                return Poll::Ready(Err(copy_io_error(err)));
            }
        }
        Poll::Ready(Ok(()))
    }
}

struct IngressState {
    index: std::sync::Mutex<IndexTrie>,
    err: std::sync::OnceLock<std::io::Error>,
    unprocessed: AtomicBool,
    timeout: std::sync::Mutex<Option<Duration>>,
}

enum IngressGuard {
    Abort { _task: AbortOnDropHandle<()> },
    Drain { _dropped: oneshot::Sender<()> },
}

/// Outgoing framed stream
pub struct Outgoing {
    tx: PollSender<(Bytes, Bytes)>,
    path: Arc<[usize]>,
    path_buf: Bytes,
    timeout: Arc<std::sync::Mutex<Timeout>>,
    err: Arc<std::sync::OnceLock<std::io::Error>>,
}

#[derive(Default)]
struct Timeout {
    duration: Option<Duration>,
    waker: Option<Waker>,
}

impl Outgoing {
    /// Creates a new [Outgoing] given an [`AsyncWrite`].
    pub fn new<T, Fut>(
        mut tx: T,
        on_egress: impl FnOnce(T, std::io::Result<()>) -> Fut + Send + 'static,
    ) -> Self
    where
        T: AsyncWrite + Unpin + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let span = Span::current();
        let (tx_tx, mut tx_rx) = mpsc::channel(128);
        let timeout = Arc::new(std::sync::Mutex::default());
        let err = Arc::new(std::sync::OnceLock::default());
        tokio::spawn({
            let timeout = Arc::clone(&timeout);
            let err = Arc::clone(&err);
            async move {
                let res = egress(
                    TimeoutWriter {
                        inner: &mut tx,
                        timeout,
                        duration: None,
                        deadline: None,
                    },
                    &mut tx_rx,
                )
                .await;
                if let Err(e) = &res {
                    _ = err.set(copy_io_error(e));
                }
                drop(tx_rx);
                on_egress(tx, res).await;
            }
            .instrument(span.clone())
        });
        Self {
            tx: PollSender::new(tx_tx),
            path: Arc::from([]),
            path_buf: Bytes::from_static(&[0]),
            timeout,
            err,
        }
    }

    /// Sets the write timeout.
    ///
    /// Writes are buffered and written to the underlying connection by a background task.
    /// Once set, that task fails with [`std::io::ErrorKind::TimedOut`] if the connection does
    /// not accept any buffered data within `timeout`, after which the result is passed to
    /// `on_egress` and all subsequent writes, flushes and shutdowns on this stream fail with
    /// the same error. Flushes and shutdowns do not wait for the background task, so they only
    /// report a timeout that has already occurred and data buffered before it is lost. Other
    /// background write errors are only reported by subsequent writes. The timeout is shared
    /// by all sub-streams of this connection, including those returned by [`Self::index`].
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        let waker = {
            let mut t = self.timeout.lock().unwrap_or_else(PoisonError::into_inner);
            t.duration = Some(timeout);
            t.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        self
    }

    /// Index the outgoing stream using a structural `path`, returning a handle that writes
    /// to the multiplexed sub-stream addressed by it.
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        let path: Arc<[usize]> = if self.path.is_empty() {
            Arc::from(path)
        } else {
            Arc::from([self.path.as_ref(), path].concat())
        };
        let mut buf = BytesMut::with_capacity(path.len().saturating_add(5));
        let n = u32::try_from(path.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding path length");
        Leb128Encoder.encode(n, &mut buf)?;
        for p in path.as_ref() {
            let p = u32::try_from(*p)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(p, "encoding path element");
            Leb128Encoder.encode(p, &mut buf)?;
        }
        Ok(Self {
            tx: self.tx.clone(),
            path,
            path_buf: buf.freeze(),
            timeout: Arc::clone(&self.timeout),
            err: Arc::clone(&self.err),
        })
    }

    pub(crate) fn poll_reserve(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.tx
            .poll_reserve(cx)
            .map_err(|e| error_egress(&self.err, e))
    }

    pub(crate) async fn reserve(&mut self) -> std::io::Result<()> {
        poll_fn(|cx| self.poll_reserve(cx)).await
    }

    pub(crate) fn send_reserved(&mut self, buf: Bytes) -> std::io::Result<()> {
        trace!(path = ?self.path, ?buf, "sending outgoing frame");
        self.tx
            .send_item((self.path_buf.clone(), buf))
            .map_err(|e| error_egress(&self.err, e))
    }

    /// Writes `buf` to the stream as a single frame, without copying it.
    pub async fn write_bytes(&mut self, buf: Bytes) -> std::io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.reserve().await?;
        self.send_reserved(buf)
    }
}

impl AsyncWrite for Outgoing {
    #[instrument(level = "trace", skip_all, fields(path = ?self.path, buf = format!("{buf:02x?}")), ret(level = "trace"))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        trace!("writing outgoing chunk");
        ready!(self.poll_reserve(cx))?;
        self.send_reserved(Bytes::copy_from_slice(buf))?;
        Poll::Ready(Ok(buf.len()))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(error_timeout(&self.err).map_or(Ok(()), Err))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(error_timeout(&self.err).map_or(Ok(()), Err))
    }
}

/// Returns a copy of the egress error, if it was a write timeout
fn error_timeout(err: &std::sync::OnceLock<std::io::Error>) -> Option<std::io::Error> {
    err.get()
        .filter(|err| err.kind() == std::io::ErrorKind::TimedOut)
        .map(copy_io_error)
}

/// Returns a copy of the egress error, if egress failed, and [`std::io::ErrorKind::BrokenPipe`]
/// wrapping `e` otherwise
fn error_egress(
    err: &std::sync::OnceLock<std::io::Error>,
    e: impl Into<Box<dyn core::error::Error + Send + Sync>>,
) -> std::io::Error {
    err.get().map_or_else(
        || std::io::Error::new(std::io::ErrorKind::BrokenPipe, e),
        copy_io_error,
    )
}

pin_project! {
    struct TimeoutWriter<T> {
        #[pin]
        inner: T,
        timeout: Arc<std::sync::Mutex<Timeout>>,
        duration: Option<Duration>,
        deadline: Option<Pin<Box<Sleep>>>,
    }
}

impl<T: AsyncWrite> TimeoutWriter<T> {
    fn poll_timeout<R>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        poll: impl FnOnce(Pin<&mut T>, &mut Context<'_>) -> Poll<std::io::Result<R>>,
    ) -> Poll<std::io::Result<R>> {
        let this = self.project();
        if let Poll::Ready(res) = poll(this.inner, cx) {
            *this.deadline = None;
            return Poll::Ready(res);
        }
        let duration = {
            let mut t = this.timeout.lock().unwrap_or_else(PoisonError::into_inner);
            if !t.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                t.waker = Some(cx.waker().clone());
            }
            t.duration
        };
        if duration != *this.duration {
            *this.duration = duration;
            *this.deadline = None;
        }
        let Some(duration) = duration else {
            return Poll::Pending;
        };
        let sleep = this
            .deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(duration)));
        ready!(sleep.as_mut().poll(cx));
        trace!("connection write timed out");
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connection write timed out",
        )))
    }
}

impl<T: AsyncWrite> AsyncWrite for TimeoutWriter<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_timeout(cx, |inner, cx| inner.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_timeout(cx, |inner, cx| inner.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_timeout(cx, AsyncWrite::poll_flush)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_timeout(cx, AsyncWrite::poll_shutdown)
    }
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
async fn ingress(
    mut rx: impl AsyncRead + Unpin,
    index: &std::sync::Mutex<IndexTrie>,
    param_tx: &mpsc::Sender<std::io::Result<Bytes>>,
) -> std::io::Result<()> {
    loop {
        trace!("reading path length");
        let b = match rx.read_u8().await {
            Ok(b) => b,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        };
        let n = AsyncReadExt::chain([b].as_slice(), &mut rx)
            .read_u32_leb128()
            .await?;
        let n = usize::try_from(n)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "read path length");
        let tx = if n == 0 {
            param_tx
        } else {
            let mut path = Vec::with_capacity(n.min(MAX_INITIAL_PATH_CAPACITY));
            for i in 0..n {
                trace!(i, "reading path element");
                let p = rx.read_u32_leb128().await?;
                let p = usize::try_from(p)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                path.push(p);
            }
            trace!(?path, "read path");

            trace!("locking index trie");
            let mut index = index
                .lock()
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            &index.get_tx(&path).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("`{path:?}` subscription not found"),
                )
            })?
        };
        trace!("reading data length");
        let n = rx.read_u32_leb128().await?;
        trace!(n, "read data length");
        let mut data = (&mut rx).take(n.into());
        while data.limit() > 0 {
            let k = usize::try_from(data.limit())
                .unwrap_or(usize::MAX)
                .min(DEFAULT_MAX_INITIAL_CAPACITY);
            let mut buf = BytesMut::with_capacity(k);
            trace!(len = k, "reading data chunk");
            while buf.len() < k {
                if data.read_buf(&mut buf).await? == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
            }
            trace!(?buf, "read data chunk");
            tx.send(Ok(buf.freeze()))
                .await
                .map_err(|_| error_receiver_closed())?;
        }
    }
}

/// Closes all receivers in the index trie once all [Incoming] handles were dropped,
/// failing if any of them or any of the dropped handles held data, which was not processed
fn close_rx(state: &IngressState) -> std::io::Result<()> {
    trace!("all handles dropped, closing index trie receivers");
    let mut index = state.index.lock().unwrap_or_else(PoisonError::into_inner);
    if index.close_rx() || state.unprocessed.load(Ordering::Relaxed) {
        return Err(error_unprocessed());
    }
    Ok(())
}

/// Returns the error for received data, which was not processed
fn error_unprocessed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "received data was not processed",
    )
}

#[instrument(level = "trace", skip_all)]
async fn egress(
    mut tx: impl AsyncWrite + Unpin,
    rx: &mut mpsc::Receiver<(Bytes, Bytes)>,
) -> std::io::Result<()> {
    let mut buf = BytesMut::with_capacity(5);
    trace!("waiting for next frame");
    let mut next = rx.recv().await;
    while let Some((path, data)) = next {
        let data_len = u32::try_from(data.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        buf.clear();
        Leb128Encoder.encode(data_len, &mut buf)?;
        let mut frame = path.chain(&mut buf).chain(data);
        trace!(?frame, "writing egress frame");
        tx.write_all_buf(&mut frame).await?;
        next = match rx.try_recv() {
            Ok(frame) => Some(frame),
            Err(..) => {
                tx.flush().await?;
                trace!("waiting for next frame");
                rx.recv().await
            }
        };
    }
    trace!("shutting down outgoing stream");
    tx.shutdown().await
}

/// Connection handler defines the connection I/O behavior.
/// It is mostly useful for transports that may require additional clean up not already covered
/// by [`AsyncWriteExt::shutdown`](tokio::io::AsyncWriteExt::shutdown), for example.
/// This API is experimental and may change in backwards-incompatible ways in the future.
pub trait ConnHandler<Rx, Tx> {
    /// Whether to keep reading once all [Incoming] handles are dropped until the peer closes
    /// the stream, rather than aborting ingress and dropping the reader.
    /// Any data received after all handles are dropped, or received for a registered path, which
    /// was never indexed, fails ingress with [`std::io::ErrorKind::BrokenPipe`].
    /// Draining is required for transports that reset the connection when closed with
    /// unread data, like TCP, discarding data not yet received by the peer.
    /// Draining is bounded by the read timeout, if one is set, see [`Incoming::with_timeout`].
    const DRAIN: bool = true;

    /// Handle ingress completion
    fn on_ingress(rx: Rx, res: std::io::Result<()>) -> impl Future<Output = ()> + Send {
        _ = rx;
        if let Err(err) = res {
            error!(?err, "ingress failed");
        } else {
            debug!("ingress successfully complete");
        }
        async {}
    }

    /// Handle egress completion
    fn on_egress(tx: Tx, res: std::io::Result<()>) -> impl Future<Output = ()> + Send {
        _ = tx;
        if let Err(err) = res {
            error!(?err, "egress failed");
        } else {
            debug!("egress successfully complete");
        }
        async {}
    }
}

impl<Rx, Tx> ConnHandler<Rx, Tx> for () {}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use std::io::ErrorKind;

    use tokio::io::{BufReader, DuplexStream};

    use super::*;

    const FRAME: [u8; 4] = [0x01, 0x00, 0x01, 0x2a];

    fn incoming(
        drain: bool,
    ) -> (
        DuplexStream,
        Incoming,
        oneshot::Receiver<std::io::Result<()>>,
    ) {
        let (tx, rx) = tokio::io::duplex(16);
        let (done_tx, done_rx) = oneshot::channel();
        let rx = Incoming::new(BufReader::new(rx), [[Some(0)]], drain, |_, res| async {
            _ = done_tx.send(res);
        });
        (tx, rx, done_rx)
    }

    async fn done(rx: oneshot::Receiver<std::io::Result<()>>) -> std::io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("ingress timed out")
            .expect("ingress aborted")
    }

    async fn failed(rx: oneshot::Receiver<std::io::Result<()>>) -> ErrorKind {
        done(rx).await.unwrap_err().kind()
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_aborts() {
        let (_tx, rx, done_rx) = incoming(false);
        drop(rx);
        tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .unwrap()
            .unwrap_err();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_waits_for_eof() {
        let (tx, rx, mut done_rx) = incoming(true);
        drop(rx);
        tokio::task::yield_now().await;
        assert!(done_rx.try_recv().is_err());
        drop(tx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_accepts_empty_frame() {
        let (mut tx, rx, done_rx) = incoming(true);
        drop(rx);
        tx.write_all(&[0x00, 0x00]).await.unwrap();
        drop(tx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_rejects_data() {
        let (mut tx, rx, done_rx) = incoming(true);
        drop(rx);
        tx.write_all(&FRAME).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_reports_ingress_error() {
        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::NotFound);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_rejects_pending() {
        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_rejects_unread() {
        let (mut tx, rx, done_rx) = incoming(true);
        let sub = rx.index(&[0]).unwrap();
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(sub);
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn buffered_drop_rejects_unread() {
        let (tx, rx, done_rx) = incoming(true);
        drop(crate::BufferedIncoming {
            buffer: BytesMut::from(&FRAME[..]),
            inner: rx,
        });
        drop(tx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_eof_waits_for_drop() {
        let (tx, mut rx, mut done_rx) = incoming(true);
        drop(tx);
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        assert!(done_rx.try_recv().is_err());
        drop(rx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_eof_rejects_pending() {
        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        drop(tx);
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_drain_times_out() {
        let (_tx, rx, done_rx) = incoming(true);
        drop(rx.with_timeout(Duration::from_millis(10)));
        assert_eq!(failed(done_rx).await, ErrorKind::TimedOut);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_read_after_timeout() {
        let (mut tx, rx, _done_rx) = incoming(false);
        let mut rx = rx.with_timeout(Duration::from_millis(10));
        let mut buf = [0; 1];
        let err = rx.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TimedOut);
        tx.write_all(&[0x00, 0x01, 0x2a]).await.unwrap();
        rx.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_timeout_after_stall() {
        let (tx, _rx) = tokio::io::duplex(1);
        let (done_tx, done_rx) = oneshot::channel();
        let mut tx = Outgoing::new(tx, |_, res| async {
            _ = done_tx.send(res);
        });
        tx.write_all(&[0; 16]).await.unwrap();
        tokio::task::yield_now().await;
        let mut tx = tx.with_timeout(Duration::from_millis(10));
        assert_eq!(failed(done_rx).await, ErrorKind::TimedOut);
        let err = tx.write_all(&[0]).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TimedOut);
        let err = tx.flush().await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TimedOut);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_truncated_path() {
        let index = std::sync::Mutex::new(IndexTrie::Empty);
        let (tx, _rx) = mpsc::channel(1);
        let err = ingress([0x02, 0x00].as_slice(), &index, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_chunked_data() {
        let index = std::sync::Mutex::new(IndexTrie::Empty);
        let (tx, mut rx) = mpsc::channel(4);
        let n = DEFAULT_MAX_INITIAL_CAPACITY + 1;
        let mut frame = vec![0x00, 0x81, 0x80, 0x40];
        frame.resize(frame.len() + n, 0x42);
        ingress(frame.as_slice(), &index, &tx).await.unwrap();
        drop(tx);
        let mut chunks = vec![];
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk.unwrap().len());
        }
        assert_eq!(chunks, [DEFAULT_MAX_INITIAL_CAPACITY, 1]);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_truncated_data() {
        let index = std::sync::Mutex::new(IndexTrie::Empty);
        let (tx, _rx) = mpsc::channel(1);
        let err = ingress(
            [0x00, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x42].as_slice(),
            &index,
            &tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
