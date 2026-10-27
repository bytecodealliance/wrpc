use core::fmt::{Debug, Display};
use core::future::poll_fn;
use core::mem;
use core::ops::Bound;
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, ready};

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

use bytes::{Buf as _, Bytes, BytesMut};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt as _};
use tokio::select;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
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

/// Sender of a subscribed path
#[derive(Debug)]
enum TxSlot {
    Open(mpsc::Sender<std::io::Result<Bytes>>),
    Closed,
}

/// Maximum number of elements, i.e. positions matched by a wildcard, which data is received for
/// before they are indexed
const MAX_UNCLAIMED_ELEMENTS: usize = 64;

/// Declared path or a prefix of one
#[derive(Clone, Copy, Debug, Default)]
struct Pattern {
    /// Whether the path is declared
    declared: bool,
    /// Whether the positions nested in the path are matched by a wildcard, if there are any
    wildcard: Option<bool>,
}

/// Subscriptions of the declared async paths and the root.
///
/// Elements, i.e. positions matched by a wildcard, are subscribed to once indexed or once data
/// is received for them, and removed once all handles within them are dropped. Indexing an
/// element releases the preceding ones, which were not subscribed to. At most
/// [`MAX_UNCLAIMED_ELEMENTS`] elements are subscribed to before being indexed, after which
/// ingress waits for them to be indexed.
#[derive(Debug)]
struct Subscriptions {
    /// Declared paths and their prefixes
    patterns: BTreeMap<Box<[Option<usize>]>, Pattern>,
    /// Subscriptions keyed by path
    subs: BTreeMap<Box<[usize]>, Subscription>,
    /// Lowest index of an element, which is not released, keyed by the path containing it
    next: BTreeMap<Box<[usize]>, usize>,
    /// Permits of elements subscribed to before being indexed
    unclaimed: Arc<Semaphore>,
    /// Notified once elements are indexed or removed
    indexed: Arc<Notify>,
    /// Whether all senders are closed
    closed: bool,
}

#[derive(Debug, Default)]
struct Subscription {
    tx: Option<TxSlot>,
    rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    /// Number of handles within an element
    handles: usize,
    /// Permit of an element subscribed to before being indexed
    permit: Option<OwnedSemaphorePermit>,
}

impl Subscription {
    /// Creates a subscription, which receives data if the path is `declared`
    fn new(declared: bool) -> Self {
        if !declared {
            return Self::default();
        }
        let (tx, rx) = mpsc::channel(16);
        Self {
            tx: Some(TxSlot::Open(tx)),
            rx: Some(rx),
            ..Self::default()
        }
    }
}

/// Destination of a received frame
enum Route<'a> {
    /// Sender slot of the subscription
    Tx(&'a mut TxSlot),
    /// The frame is for an element, which was released
    Released,
    /// The frame is for an element, which can only be subscribed to using a permit
    Unclaimed,
}

impl Subscriptions {
    /// Declares `paths`, with the data of the root sent to `tx`, ignoring the paths, which
    /// declare both a wildcard and an index at the same position as a preceding one
    fn new<P: AsRef<[Option<usize>]>>(
        paths: impl IntoIterator<Item = P>,
        tx: mpsc::Sender<std::io::Result<Bytes>>,
    ) -> Self {
        let mut subs = Self {
            patterns: BTreeMap::from([(
                Box::default(),
                Pattern {
                    declared: true,
                    wildcard: None,
                },
            )]),
            subs: BTreeMap::from([(
                Box::default(),
                Subscription {
                    tx: Some(TxSlot::Open(tx)),
                    ..Subscription::default()
                },
            )]),
            next: BTreeMap::default(),
            unclaimed: Arc::new(Semaphore::new(MAX_UNCLAIMED_ELEMENTS)),
            indexed: Arc::default(),
            closed: false,
        };
        'paths: for path in paths {
            let path = path.as_ref();
            if path.is_empty() {
                continue;
            }
            for (k, i) in path.iter().enumerate() {
                let parent = subs.patterns.entry(Box::from(&path[..k])).or_default();
                if *parent.wildcard.get_or_insert(i.is_none()) != i.is_none() {
                    continue 'paths;
                }
            }
            subs.patterns.entry(Box::from(path)).or_default().declared = true;
            if let Some(path) = path.iter().copied().collect() {
                subs.subs.insert(path, Subscription::new(true));
            }
        }
        subs
    }

    /// Resolves `path` to the declared path or prefix matching it
    fn resolve(&self, path: &[usize]) -> Option<(Pattern, Vec<Option<usize>>)> {
        let mut pattern = Vec::with_capacity(path.len());
        let mut node = self.patterns.get::<[_]>(&[])?;
        for i in path {
            pattern.push(if node.wildcard? { None } else { Some(*i) });
            node = self.patterns.get(pattern.as_slice())?;
        }
        Some((*node, pattern))
    }

    /// Returns the lowest index of an element in `path`, which is not released
    fn next(&self, path: &[usize]) -> usize {
        self.next.get(path).copied().unwrap_or_default()
    }

    /// Subscribes to `path` matching `pattern`, if it is not subscribed to yet
    fn subscribe(&mut self, path: &[usize], pattern: &[Option<usize>]) -> &mut Subscription {
        let declared = self.patterns.get(pattern).is_some_and(|p| p.declared);
        let closed = self.closed;
        self.subs.entry(Box::from(path)).or_insert_with(|| {
            let mut sub = Subscription::new(declared);
            if closed {
                sub.tx = None;
            }
            sub
        })
    }

    /// Closes all receivers, returning `true` if any of them held data
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn close_rx(&mut self) -> bool {
        self.unclaimed.close();
        let mut pending = false;
        for sub in self.subs.values_mut() {
            pending |= sub.rx.take().is_some_and(|rx| !rx.is_empty());
        }
        pending
    }

    /// Closes all open senders
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn close_tx(&mut self) {
        self.closed = true;
        for sub in self.subs.values_mut() {
            if let Some(TxSlot::Open(..)) = sub.tx {
                sub.tx.take();
            }
        }
    }

    /// Takes the receiver for a new handle, returning `None` if neither `path` nor any path
    /// nested under it is subscribed to
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn take_rx(
        &mut self,
        path: &[usize],
    ) -> Option<Option<mpsc::Receiver<std::io::Result<Bytes>>>> {
        let (Pattern { declared, wildcard }, pattern) = self.resolve(path)?;
        for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
            if !self.subs.contains_key(&path[..=k]) && path[k] < self.next(&path[..k]) {
                return None;
            }
        }
        if declared && wildcard.is_none() && self.subs.get(path).is_some_and(|sub| sub.rx.is_none())
        {
            return None;
        }
        for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
            let element = self.subscribe(&path[..=k], &pattern[..=k]);
            element.handles += 1;
            element.permit = None;
            let next = self.next.entry(Box::from(&path[..k])).or_default();
            *next = (*next).max(path[k].saturating_add(1));
            self.indexed.notify_one();
        }
        if !declared {
            return Some(None);
        }
        Some(self.subscribe(path, &pattern).rx.take())
    }

    /// Accounts for a dropped handle of `path`, removing the elements without handles,
    /// returning `true` if any of them held data, which was not processed
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn release(&mut self, path: &[usize]) -> bool {
        let Some((.., pattern)) = self.resolve(path) else {
            return false;
        };
        for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
            let Some(element) = self.subs.get_mut(&path[..=k]) else {
                return false;
            };
            element.handles = element.handles.saturating_sub(1);
            if element.handles == 0 {
                return self.remove(&path[..=k]);
            }
        }
        false
    }

    /// Removes the element at `path` and everything nested in it, returning `true` if any of
    /// the removed subscriptions held data, which was not processed
    fn remove(&mut self, path: &[usize]) -> bool {
        remove_nested(&mut self.next, path);
        self.indexed.notify_one();
        remove_nested(&mut self.subs, path)
            .into_iter()
            .any(|sub| sub.rx.is_some_and(|rx| !rx.is_empty()))
    }

    /// Routes a frame received on `path`, subscribing to the elements in it using `permit`
    fn route(
        &mut self,
        path: &[usize],
        permit: &mut Option<OwnedSemaphorePermit>,
    ) -> std::io::Result<Route<'_>> {
        if self.subs.get(path).is_none_or(|sub| sub.tx.is_none()) {
            let Some((Pattern { declared: true, .. }, pattern)) = self.resolve(path) else {
                return Err(error_not_found(path));
            };
            for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
                if self.subs.contains_key(&path[..=k]) {
                    continue;
                }
                if path[k] < self.next(&path[..k]) {
                    return Ok(Route::Released);
                }
                let Some(permit) = permit.take() else {
                    return Ok(Route::Unclaimed);
                };
                trace!(path = ?&path[..=k], "subscribing to unclaimed element");
                self.subscribe(&path[..=k], &pattern[..=k]).permit = Some(permit);
            }
            self.subscribe(path, &pattern);
        }
        self.get_tx_mut(path)
            .map(Route::Tx)
            .ok_or_else(|| error_not_found(path))
    }

    /// Gets the sender slot of a subscription
    #[instrument(level = "trace", skip(self))]
    fn get_tx_mut(&mut self, path: &[usize]) -> Option<&mut TxSlot> {
        let sub = self.subs.get_mut(path)?;
        sub.tx.as_mut()
    }
}

/// Removes the entries of `map` at `path` and nested in it
fn remove_nested<V>(map: &mut BTreeMap<Box<[usize]>, V>, path: &[usize]) -> Vec<V> {
    let keys: Vec<_> = map
        .range::<[usize], _>((Bound::Included(path), Bound::Unbounded))
        .map(|(p, ..)| p)
        .take_while(|p| p.starts_with(path))
        .cloned()
        .collect();
    keys.iter().filter_map(|p| map.remove(p)).collect()
}

pin_project! {
    /// Incoming framed stream
    #[project = IncomingProj]
    pub struct Incoming {
        #[pin]
        rx: Option<StreamReader<ReceiverStream<std::io::Result<Bytes>>, Bytes>>,
        subscribed: bool,
        path: Arc<[usize]>,
        io: Arc<IngressGuard>,
        state: Arc<IngressState>,
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
            if !this.path.is_empty() {
                let mut index = this.state.index.lock().unwrap_or_else(PoisonError::into_inner);
                if index.release(&this.path) {
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
        let index = Subscriptions::new(paths, rx_tx);
        let state = Arc::new(IngressState {
            index: std::sync::Mutex::new(index),
            err: std::sync::OnceLock::default(),
            unprocessed: AtomicBool::default(),
        });
        let (dropped_tx, dropped_rx) = drain.then(oneshot::channel).unzip();
        let span = Span::current();
        let task = tokio::spawn({
            let state = Arc::clone(&state);
            async move {
                let res = async {
                    let mut io = pin!(async {
                        let res = ingress(&mut rx, &state.index).await;
                        if let Err(err) = &res {
                            _ = state.err.set(copy_io_error(err));
                        }
                        trace!("shutting down subscriptions");
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
                            io.await
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
            subscribed: true,
            path: Arc::from([]),
            io: Arc::new(match dropped_tx {
                // TODO: Bound draining by a deadline or a byte limit
                Some(dropped) => IngressGuard::Drain { _dropped: dropped },
                None => IngressGuard::Abort {
                    _task: AbortOnDropHandle::new(task),
                },
            }),
            state,
        }
    }

    /// Marks data received by this handle as not processed, which fails ingress once all
    /// handles are dropped
    pub(crate) fn mark_unprocessed(&self) {
        trace!(path = ?self.path, "dropping handle with unprocessed data");
        self.state.unprocessed.store(true, Ordering::Relaxed);
    }

    /// Index the incoming stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it.
    ///
    /// Elements, i.e. positions matched by a wildcard in the async paths, must be indexed in
    /// order, since indexing an element releases the preceding ones, which were not indexed yet.
    /// An element is released once all handles within it are dropped.
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
        trace!("locking subscriptions");
        let mut index = self
            .state
            .index
            .lock()
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        trace!(?path, "taking index subscription");
        let rx = index.take_rx(&path).ok_or_else(|| error_not_found(&path))?;
        Ok(Self {
            subscribed: rx.is_some(),
            rx: rx.map(|rx| StreamReader::new(ReceiverStream::new(rx))),
            path,
            io: Arc::clone(&self.io),
            state: Arc::clone(&self.state),
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
            if !*this.subscribed {
                return Poll::Ready(Err(error_not_found(this.path)));
            }
            trace!("reader is closed");
            return Poll::Ready(Ok(()));
        };
        ready!(rx.poll_read(cx, buf))?;
        trace!(buf = ?buf.filled(), "read buffer");
        if buf.filled().is_empty() {
            self.rx.take();
            if let Some(err) = self.state.err.get()
                && !self.state.is_closed(&self.path)
            {
                return Poll::Ready(Err(copy_io_error(err)));
            }
        }
        Poll::Ready(Ok(()))
    }
}

struct IngressState {
    index: std::sync::Mutex<Subscriptions>,
    err: std::sync::OnceLock<std::io::Error>,
    unprocessed: AtomicBool,
}

impl IngressState {
    /// Returns whether an empty frame closed the `path`
    fn is_closed(&self, path: &[usize]) -> bool {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        matches!(index.get_tx_mut(path), Some(TxSlot::Closed))
    }
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
        let (tx_tx, tx_rx) = mpsc::channel(128);
        tokio::spawn(
            async {
                let res = egress(&mut tx, tx_rx).await;
                on_egress(tx, res).await;
            }
            .instrument(span.clone()),
        );
        Self {
            tx: PollSender::new(tx_tx),
            path: Arc::from([]),
            path_buf: Bytes::from_static(&[0]),
        }
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
        })
    }

    pub(crate) fn poll_reserve(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.tx
            .poll_reserve(cx)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err))
    }

    pub(crate) async fn reserve(&mut self) -> std::io::Result<()> {
        poll_fn(|cx| self.poll_reserve(cx)).await
    }

    pub(crate) fn send_reserved(&mut self, buf: Bytes) -> std::io::Result<()> {
        trace!(path = ?self.path, ?buf, "sending outgoing frame");
        self.tx
            .send_item((self.path_buf.clone(), buf))
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err))
    }

    /// Writes `buf` to the stream as a single frame, without copying it.
    /// Empty `buf` is not written, since an empty frame closes the stream, see [`Self::close`].
    pub async fn write_bytes(&mut self, buf: Bytes) -> std::io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.reserve().await?;
        self.send_reserved(buf)
    }

    /// Closes the stream by writing an empty frame. No more data may be written to it afterwards.
    pub async fn close(&mut self) -> std::io::Result<()> {
        self.reserve().await?;
        self.send_reserved(Bytes::default())
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
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
async fn ingress(
    mut rx: impl AsyncRead + Unpin,
    index: &std::sync::Mutex<Subscriptions>,
) -> std::io::Result<()> {
    'frames: loop {
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
        let path = if n == 0 {
            Vec::default()
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
            path
        };
        trace!("reading data length");
        let n = rx.read_u32_leb128().await?;
        trace!(n, "read data length");
        let mut permit = None;
        let tx = loop {
            let (unclaimed, indexed) = {
                trace!("locking subscriptions");
                let mut subs = index
                    .lock()
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                match subs.route(&path, &mut permit)? {
                    Route::Tx(tx) => {
                        if n == 0 {
                            trace!(?path, "closing path");
                            if let TxSlot::Closed = mem::replace(tx, TxSlot::Closed) {
                                return Err(error_closed(&path));
                            }
                            continue 'frames;
                        }
                        let TxSlot::Open(tx) = tx else {
                            return Err(error_closed(&path));
                        };
                        break tx.clone();
                    }
                    Route::Released if n == 0 => continue 'frames,
                    Route::Released => return Err(error_receiver_closed()),
                    Route::Unclaimed => (Arc::clone(&subs.unclaimed), Arc::clone(&subs.indexed)),
                }
            };
            trace!(?path, "waiting for elements to be indexed");
            select! {
                res = unclaimed.acquire_owned() => match res {
                    Ok(p) => permit = Some(p),
                    Err(..) if n == 0 => continue 'frames,
                    Err(..) => return Err(error_receiver_closed()),
                },
                () = indexed.notified() => {}
            }
        };
        drop(permit);
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

/// Closes all receivers of the subscriptions once all [Incoming] handles were dropped,
/// failing if any of them or any of the dropped handles held data, which was not processed
fn close_rx(state: &IngressState) -> std::io::Result<()> {
    trace!("all handles dropped, closing subscription receivers");
    let mut index = state.index.lock().unwrap_or_else(PoisonError::into_inner);
    if index.close_rx() || state.unprocessed.load(Ordering::Relaxed) {
        return Err(error_unprocessed());
    }
    Ok(())
}

/// Returns the error for a frame received on a closed `path`
fn error_closed(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("received a frame on closed path `{path:?}`"),
    )
}

/// Returns the error for a `path`, which is not subscribed to
fn error_not_found(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("`{path:?}` subscription not found"),
    )
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
    mut rx: mpsc::Receiver<(Bytes, Bytes)>,
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

    use futures::FutureExt as _;
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
        incoming_paths(drain, [[Some(0)]])
    }

    fn incoming_paths<P: AsRef<[Option<usize>]>>(
        drain: bool,
        paths: impl IntoIterator<Item = P>,
    ) -> (
        DuplexStream,
        Incoming,
        oneshot::Receiver<std::io::Result<()>>,
    ) {
        let (tx, rx) = tokio::io::duplex(16);
        let (done_tx, done_rx) = oneshot::channel();
        let rx = Incoming::new(BufReader::new(rx), paths, drain, |_, res| async {
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
    async fn incoming_rejects_unsubscribed_paths() {
        let (_tx, rx, ..) =
            incoming_paths(false, [&[Some(0)][..], &[Some(1)], &[Some(1), Some(0)]]);
        rx.index(&[0]).unwrap();
        for path in [&[0][..], &[2], &[1, 1]] {
            let Err(err) = rx.index(path) else {
                panic!("indexing `{path:?}` should fail");
            };
            assert_eq!(err.kind(), ErrorKind::NotFound);
        }
        let (_tx, rx, ..) = incoming_paths(false, [[Some(0), Some(0)]]);
        let mut sub = rx.index(&[0]).unwrap();
        sub.index(&[0]).unwrap();
        let err = sub.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
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
        let (mut tx, rx, mut done_rx) = incoming(true);
        drop(rx);
        tokio::task::yield_now().await;
        assert!(done_rx.try_recv().is_err());
        tx.write_all(&[0x00, 0x00]).await.unwrap();
        tokio::task::yield_now().await;
        assert!(done_rx.try_recv().is_err());
        drop(tx);
        done(done_rx).await.unwrap();
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
    async fn incoming_empty_frame_closes_path() {
        let (mut tx, mut rx, done_rx) = incoming(true);
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&[0x00, 0x00]).await.unwrap();
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0x00, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::NotFound);
        let mut buf = vec![];
        sub.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        assert_eq!(rx.read_to_end(&mut Vec::default()).await.unwrap(), 0);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_unprocessed_data() {
        let (mut tx, rx, done_rx) = incoming(true);
        drop(rx);
        tx.write_all(&FRAME).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming(true);
        let sub = rx.index(&[0]).unwrap();
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(sub);
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (tx, rx, done_rx) = incoming(true);
        drop(crate::BufferedIncoming {
            buffer: BytesMut::from(&FRAME[..]),
            inner: rx,
        });
        drop(tx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        drop(tx);
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming(true);
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::NotFound);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_declared_paths() {
        let (mut tx, rx, done_rx) = incoming_paths(
            true,
            [
                &[Some(0)][..],
                &[Some(0)],
                &[Some(0xffff_ffff)],
                &[Some(1), None],
                &[Some(1), Some(0)],
            ],
        );
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x01, 0x2b])
            .await
            .unwrap();
        tx.write_all(&[0x02, 0x01, 0x05, 0x01, 0x2c]).await.unwrap();
        drop(tx);
        for (path, data) in [(&[0][..], 0x2a), (&[0xffff_ffff], 0x2b), (&[1, 5], 0x2c)] {
            let mut buf = Vec::default();
            rx.index(path).unwrap().read_to_end(&mut buf).await.unwrap();
            assert_eq!(buf, [data]);
        }
        drop(rx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_invalid_frames() {
        for (frames, kind) in [
            ([0x02, 0x00].as_slice(), ErrorKind::UnexpectedEof),
            (
                &[0x00, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x42],
                ErrorKind::UnexpectedEof,
            ),
            (&[0x00, 0x00, 0x00, 0x01, 0x2a], ErrorKind::InvalidData),
            (&[0x00, 0x00, 0x00, 0x00], ErrorKind::InvalidData),
            (
                &[0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x2a],
                ErrorKind::InvalidData,
            ),
            (
                &[0x01, 0x00, 0x00, 0x01, 0x00, 0x00],
                ErrorKind::InvalidData,
            ),
            (&[0x01, 0x01, 0x00], ErrorKind::NotFound),
            (&[0x01, 0x01, 0x01, 0x2a], ErrorKind::NotFound),
            (&[0x03, 0x02, 0x03, 0x00, 0x01, 0x2a], ErrorKind::NotFound),
        ] {
            let (mut tx, _rx, done_rx) = incoming_paths(
                true,
                [[Some(0)].as_slice(), &[Some(1), Some(0)], &[Some(2), None]],
            );
            tx.write_all(frames).await.unwrap();
            drop(tx);
            assert_eq!(failed(done_rx).await, kind, "{frames:02x?}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_wildcard_elements() {
        let (mut tx, rx, done_rx) = incoming_paths(true, [[None]]);
        tx.write_all(&[0x01, 0x03, 0x01, 0x2b]).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        let mut buf = [0; 1];
        rx.index(&[1]).unwrap().read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        let mut sub = rx.index(&[3]).unwrap();
        sub.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2b]);
        let Err(err) = rx.index(&[2]) else {
            panic!("indexing a released element should fail");
        };
        assert_eq!(err.kind(), ErrorKind::NotFound);
        drop(sub);
        tx.write_all(&[0x01, 0x03, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x03, 0x01, 0x2c]).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            rx.index(&[4]).unwrap().read_to_end(&mut Vec::default()),
        )
        .await
        .expect("read timed out")
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_nested_wildcard_elements() {
        let (mut tx, rx, done_rx) = incoming_paths(true, [[None, Some(0), None]]);
        let element = rx.index(&[1]).unwrap();
        let mut buf = [0; 1];
        for i in 0..100 {
            tx.write_all(&[0x03, 0x01, 0x00, i, 0x01, i]).await.unwrap();
            let mut sub = element.index(&[0, i.into()]).unwrap();
            sub.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, [i]);
            drop(sub);
            let index = rx.state.index.lock().unwrap();
            assert_eq!(index.subs.len(), 2, "{index:?}");
            assert_eq!(index.next.len(), 2, "{index:?}");
        }
        drop(element);
        {
            let index = rx.state.index.lock().unwrap();
            assert_eq!(index.subs.len(), 1, "{index:?}");
            assert_eq!(index.next.len(), 1, "{index:?}");
        }
        drop(rx);
        drop(tx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_released_element_fails_unclaimed_frame() {
        let (mut tx, rx, done_rx) = incoming_paths(true, [[None, None]]);
        let element = rx.index(&[0]).unwrap();
        for i in 1..=32 {
            tx.write_all(&[0x02, i, 0x00, 0x01, 0x2a]).await.unwrap();
        }
        tx.write_all(&[0x02, 0x00, 0x05, 0x01, 0x2a]).await.unwrap();
        tokio::task::yield_now().await;
        drop(element);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_waits_for_unclaimed_elements() {
        let (tx, _rx) = mpsc::channel(1);
        let index = std::sync::Mutex::new(Subscriptions::new([[None]], tx));
        let frames: Vec<u8> = (1..=MAX_UNCLAIMED_ELEMENTS)
            .chain([0, MAX_UNCLAIMED_ELEMENTS + 1])
            .flat_map(|i| [0x01, u8::try_from(i).unwrap(), 0x01, 0x2a])
            .collect();
        let mut ingress = pin!(tokio::task::unconstrained(ingress(
            frames.as_slice(),
            &index
        )));
        assert!(ingress.as_mut().now_or_never().is_none());
        let mut rx = index.lock().unwrap().take_rx(&[0]).unwrap().unwrap();
        assert!(ingress.as_mut().now_or_never().is_none());
        assert_eq!(rx.try_recv().unwrap().unwrap(), [0x2a].as_slice());
        let rx = index.lock().unwrap().take_rx(&[1]).unwrap();
        tokio::time::timeout(Duration::from_secs(5), ingress)
            .await
            .expect("ingress timed out")
            .unwrap();
        drop(rx);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_chunked_data() {
        let (tx, mut rx) = mpsc::channel(4);
        let index = std::sync::Mutex::new(Subscriptions::new::<[_; 0]>([], tx));
        let n = DEFAULT_MAX_INITIAL_CAPACITY + 1;
        let mut frame = vec![0x00, 0x81, 0x80, 0x40];
        frame.resize(frame.len() + n, 0x42);
        ingress(frame.as_slice(), &index).await.unwrap();
        drop(index);
        let mut chunks = vec![];
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk.unwrap().len());
        }
        assert_eq!(chunks, [DEFAULT_MAX_INITIAL_CAPACITY, 1]);
    }
}
