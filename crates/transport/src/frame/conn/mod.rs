use core::fmt::{Debug, Display};
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};
use core::time::Duration;

use std::sync::{Arc, PoisonError};

use bytes::{Buf as _, Bytes, BytesMut};
use futures::Sink as _;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Sleep;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tracing::{Instrument as _, Span, debug, error, instrument, trace};
use wasm_tokio::{
    AsyncReadCore as _, AsyncReadLeb128 as _, DEFAULT_MAX_INITIAL_CAPACITY, Leb128Encoder,
};

use crate::frame::MAX_INITIAL_PATH_CAPACITY;

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
        match self {
            Self::Empty => {}
            Self::Leaf { tx, .. } => {
                mem::take(tx);
            }
            Self::IndexNode { tx, nested, .. } => {
                mem::take(tx);
                for nested in nested.iter_mut().flatten() {
                    nested.close_tx();
                }
            }
            Self::WildcardNode { tx, nested, .. } => {
                mem::take(tx);
                if let Some(nested) = nested {
                    nested.close_tx();
                }
            }
        }
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
        index: Arc<std::sync::Mutex<IndexTrie>>,
        io: Arc<JoinSet<()>>,
        err: Arc<std::sync::OnceLock<std::io::Error>>,
        timeout: Option<Duration>,
        deadline: Option<Pin<Box<Sleep>>>,
    }
}

impl Incoming {
    /// Creates a new [Incoming] given an [`AsyncRead`], [`ConnHandler`] and a set of async paths.
    /// `on_ingress` will be called once data ingress is complete.
    pub fn new<T, P, Fut>(
        mut rx: T,
        paths: impl IntoIterator<Item = P>,
        on_ingress: impl FnOnce(T, std::io::Result<()>) -> Fut + Send + 'static,
    ) -> Self
    where
        T: AsyncRead + Unpin + Send + 'static,
        P: AsRef<[Option<usize>]>,
        Fut: Future<Output = ()> + Send,
    {
        let index = Arc::new(std::sync::Mutex::new(paths.into_iter().collect()));
        let (rx_tx, rx_rx) = mpsc::channel(128);
        let err = Arc::new(std::sync::OnceLock::default());
        let mut rx_io = JoinSet::new();
        let span = Span::current();
        rx_io.spawn({
            let index = Arc::clone(&index);
            let err = Arc::clone(&err);
            async move {
                let res = ingress(&mut rx, &index, &rx_tx).await;
                if let Err(e) = &res {
                    _ = err.set(copy_io_error(e));
                }
                drop(rx_tx);
                on_ingress(rx, res).await;
                let Ok(mut index) = index.lock() else {
                    error!("failed to lock index trie");
                    return;
                };
                trace!("shutting down index trie");
                index.close_tx();
            }
            .instrument(span.clone())
        });
        Self {
            rx: Some(StreamReader::new(ReceiverStream::new(rx_rx))),
            path: Arc::from([]),
            index: Arc::clone(&index),
            io: Arc::new(rx_io),
            err,
            timeout: None,
            deadline: None,
        }
    }

    /// Sets the read timeout.
    ///
    /// Once set, a read that stays pending for longer than `timeout` fails with
    /// [`std::io::ErrorKind::TimedOut`]. The deadline is armed when a read first returns
    /// [`Poll::Pending`] and cleared once a read completes or times out. Dropping a pending
    /// read does not clear it, so the next read fails as soon as the deadline elapses unless
    /// data arrives first. The timeout is inherited by sub-streams returned by [`Self::index`]
    /// after this call, including those backing async `stream` and `future` values, which must
    /// therefore not idle for longer than `timeout`; sub-streams indexed before this call are
    /// unaffected.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
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
            index: Arc::clone(&self.index),
            io: Arc::clone(&self.io),
            err: Arc::clone(&self.err),
            timeout: self.timeout,
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
            let Some(timeout) = *this.timeout else {
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
            if let Some(err) = self.err.get() {
                return Poll::Ready(Err(copy_io_error(err)));
            }
        }
        Poll::Ready(Ok(()))
    }
}

pin_project! {
    /// Outgoing framed stream
    #[project = OutgoingProj]
    pub struct Outgoing {
        #[pin]
        tx: PollSender<(Bytes, Bytes)>,
        path: Arc<[usize]>,
        path_buf: Bytes,
        timeout: Arc<std::sync::Mutex<Timeout>>,
        err: Arc<std::sync::OnceLock<std::io::Error>>,
    }
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
}

impl AsyncWrite for Outgoing {
    #[instrument(level = "trace", skip_all, fields(path = ?self.path, buf = format!("{buf:02x?}")), ret(level = "trace"))]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("writing outgoing chunk");
        let mut this = self.project();
        ready!(this.tx.as_mut().poll_ready(cx)).map_err(|e| egress_error(this.err, e))?;
        this.tx
            .start_send((this.path_buf.clone(), Bytes::copy_from_slice(buf)))
            .map_err(|e| egress_error(this.err, e))?;
        Poll::Ready(Ok(buf.len()))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(timeout_error(&self.err).map_or(Ok(()), Err))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(timeout_error(&self.err).map_or(Ok(()), Err))
    }
}

fn timeout_error(err: &std::sync::OnceLock<std::io::Error>) -> Option<std::io::Error> {
    err.get()
        .filter(|err| err.kind() == std::io::ErrorKind::TimedOut)
        .map(copy_io_error)
}

fn egress_error(
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

fn copy_io_error(err: &std::io::Error) -> std::io::Error {
    std::io::Error::new(err.kind(), err.to_string())
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
            tx.send(Ok(buf.freeze())).await.map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream receiver closed")
            })?;
        }
    }
}

#[instrument(level = "trace", skip_all)]
async fn egress(
    mut tx: impl AsyncWrite + Unpin,
    rx: &mut mpsc::Receiver<(Bytes, Bytes)>,
) -> std::io::Result<()> {
    let mut buf = BytesMut::with_capacity(5);
    trace!("waiting for next frame");
    while let Some((path, data)) = rx.recv().await {
        let data_len = u32::try_from(data.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        buf.clear();
        Leb128Encoder.encode(data_len, &mut buf)?;
        let mut frame = path.chain(&mut buf).chain(data);
        trace!(?frame, "writing egress frame");
        tx.write_all_buf(&mut frame).await?;
        tx.flush().await?;
    }
    trace!("shutting down outgoing stream");
    tx.shutdown().await
}

/// Connection handler defines the connection I/O behavior.
/// It is mostly useful for transports that may require additional clean up not already covered
/// by [`AsyncWrite::shutdown`], for example.
/// This API is experimental and may change in backwards-incompatible ways in the future.
pub trait ConnHandler<Rx, Tx> {
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
    use super::*;

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
    async fn incoming_read_after_timeout() {
        let (mut tx, rx) = tokio::io::duplex(16);
        let mut rx =
            Incoming::new(rx, [[]; 0], |_, _| async {}).with_timeout(Duration::from_millis(50));
        let mut buf = [0; 1];
        let err = rx.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            tx.write_all(&[0x00, 0x01, 0x2a]).await.unwrap();
            tx
        });
        rx.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_timeout_after_stall() {
        let (tx, _rx) = tokio::io::duplex(1);
        let (res_tx, res_rx) = tokio::sync::oneshot::channel();
        let mut out = Outgoing::new(tx, |_, res| async {
            _ = res_tx.send(res);
        });
        out.write_all(&[0; 16]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut out = out.with_timeout(Duration::from_millis(10));
        let err = tokio::time::timeout(Duration::from_secs(5), res_rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let err = out.write_all(&[0]).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_max_timeout() {
        let (tx, _rx) = tokio::io::duplex(1);
        let mut out = Outgoing::new(tx, |_, _| async {}).with_timeout(Duration::MAX);
        out.write_all(&[0; 16]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(out.err.get().is_none());
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
