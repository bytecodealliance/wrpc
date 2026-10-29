use core::fmt::{Debug, Display};
use core::future::poll_fn;
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
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tokio_util::task::AbortOnDropHandle;
use tracing::{Instrument as _, Span, debug, error, instrument, trace};
use wasm_tokio::{
    AsyncReadCore as _, AsyncReadLeb128 as _, DEFAULT_MAX_INITIAL_CAPACITY, Leb128Encoder,
};

use crate::frame::{MAX_INITIAL_PATH_CAPACITY, PROTOCOL};
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
            PROTOCOL => {
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

/// Maximum number of data chunks buffered per sub-stream
const MAX_BUFFERED_CHUNKS: usize = 16;

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
/// element releases the preceding ones, which were not indexed yet. At most
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
        let (tx, rx) = mpsc::channel(MAX_BUFFERED_CHUNKS);
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
    /// The frame is for a path, which carries no pending value
    NotFound,
    /// The frame is for an element, which was released
    Released,
    /// The frame is for an element, which can only be subscribed to using a permit
    Unclaimed,
}

impl Subscriptions {
    /// Declares `paths`, with the data of the root sent to `tx`
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
        for path in paths {
            let path = path.as_ref();
            if path.is_empty() {
                continue;
            }
            for (k, i) in path.iter().enumerate() {
                subs.patterns
                    .entry(Box::from(&path[..k]))
                    .or_default()
                    .wildcard = Some(i.is_none());
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

    /// Removes the elements preceding the ones in `path`, which were subscribed to, but not
    /// indexed, returning `true` if any of them held data, which was not processed
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn skip(&mut self, path: &[usize]) -> bool {
        let Some((.., pattern)) = self.resolve(path) else {
            return false;
        };
        let mut pending = false;
        for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
            let next = self.next(&path[..k]);
            if next >= path[k] {
                continue;
            }
            let start = [&path[..k], &[next]].concat();
            let skipped: Vec<_> = self
                .subs
                .range::<[usize], _>((
                    Bound::Included(start.as_slice()),
                    Bound::Excluded(&path[..=k]),
                ))
                .filter(|(p, sub)| p.len() == k + 1 && sub.handles == 0)
                .map(|(p, ..)| p.clone())
                .collect();
            for p in skipped {
                pending |= self.remove(&p);
            }
        }
        pending
    }

    /// Accounts for a new handle of `path`, which is already indexed
    #[instrument(level = "trace", skip(self))]
    fn retain(&mut self, path: &[usize]) {
        let Some((.., pattern)) = self.resolve(path) else {
            return;
        };
        for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
            if let Some(element) = self.subs.get_mut(&path[..=k]) {
                element.handles += 1;
            }
        }
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

    /// Routes a frame received on `path`, subscribing to the elements in it using `permit`.
    /// Nothing is subscribed to for a `path`, which is not declared.
    fn route(&mut self, path: &[usize], permit: &mut Option<OwnedSemaphorePermit>) -> Route<'_> {
        if self.subs.get(path).is_none_or(|sub| sub.tx.is_none()) {
            let Some((Pattern { declared: true, .. }, pattern)) = self.resolve(path) else {
                return Route::NotFound;
            };
            for (k, ..) in pattern.iter().enumerate().filter(|(.., p)| p.is_none()) {
                if self.subs.contains_key(&path[..=k]) {
                    continue;
                }
                if path[k] < self.next(&path[..k]) {
                    return Route::Released;
                }
                let Some(permit) = permit.take() else {
                    return Route::Unclaimed;
                };
                trace!(path = ?&path[..=k], "subscribing to unclaimed element");
                self.subscribe(&path[..=k], &pattern[..=k]).permit = Some(permit);
            }
            self.subscribe(path, &pattern);
        }
        self.get_tx_mut(path).map_or(Route::NotFound, Route::Tx)
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
        dropped: Arc<oneshot::Sender<()>>,
        task: Arc<AbortOnDropHandle<()>>,
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
    /// Creates a new [Incoming] given an [`AsyncBufRead`] and a set of async paths.
    ///
    /// Data ingress is aborted and the reader is dropped once all handles and all futures
    /// returned by [`Self::finished`] are dropped. Data ingress continues after all handles are
    /// dropped until the peer closes the stream, i.e. it is drained, as long as a future returned
    /// by [`Self::finished`] is held.
    /// Any data received for a registered path, which was not processed, fails ingress with
    /// [`std::io::ErrorKind::BrokenPipe`] once all handles are dropped, as does any data
    /// received after that while draining.
    /// Draining reports the latter, which would otherwise go unnoticed, discards empty frames the
    /// peer may still send to close paths, and reads the stream to its end, which transports
    /// reusing the connection, like HTTP/1, require.
    ///
    /// `paths` must not contain both a wildcard and an index at the same position, so paths of
    /// different variant cases, which are not indexed, must be merged by replacing such indices
    /// by wildcards.
    pub fn new<T, P>(mut rx: T, paths: impl IntoIterator<Item = P>) -> Self
    where
        T: AsyncBufRead + Unpin + Send + 'static,
        P: AsRef<[Option<usize>]>,
    {
        let (rx_tx, rx_rx) = mpsc::channel(MAX_BUFFERED_CHUNKS);
        let index = Subscriptions::new(paths, rx_tx);
        let state = Arc::new(IngressState {
            index: std::sync::Mutex::new(index),
            err: std::sync::OnceLock::default(),
            unprocessed: AtomicBool::default(),
            done: watch::Sender::new(None),
        });
        let (dropped_tx, mut dropped_rx) = oneshot::channel();
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
                    select! {
                        biased;

                        res = &mut io => {
                            res?;
                            _ = dropped_rx.await;
                            close_rx(&state)
                        }
                        _ = &mut dropped_rx => {
                            close_rx(&state)?;
                            io.await
                        }
                    }
                }
                .await;
                if let Err(err) = &res {
                    error!(?err, "ingress failed");
                } else {
                    debug!("ingress successfully complete");
                }
                state.done.send_replace(Some(res));
            }
            .instrument(span.clone())
        });
        Self {
            rx: Some(StreamReader::new(ReceiverStream::new(rx_rx))),
            subscribed: true,
            path: Arc::from([]),
            dropped: Arc::new(dropped_tx),
            task: Arc::new(AbortOnDropHandle::new(task)),
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
        if index.skip(&path) {
            self.mark_unprocessed();
        }
        trace!(?path, "taking index subscription");
        let rx = index.take_rx(&path).ok_or_else(|| error_not_found(&path))?;
        Ok(Self {
            subscribed: rx.is_some(),
            rx: rx.map(|rx| StreamReader::new(ReceiverStream::new(rx))),
            path,
            dropped: Arc::clone(&self.dropped),
            task: Arc::clone(&self.task),
            state: Arc::clone(&self.state),
        })
    }

    /// Returns a future, which resolves to the result of ingress once it fails, or once it is
    /// complete and all handles are dropped. Ingress is drained while the future is held, see
    /// [`Self::new`].
    pub fn finished(&self) -> impl Future<Output = std::io::Result<()>> + Send + 'static {
        let task = Arc::clone(&self.task);
        let done = finished(self.state.done.subscribe());
        async move {
            let _task = task;
            done.await
        }
    }

    /// Takes the data of this handle, returning a handle reading it, if this handle reads any.
    /// This handle keeps addressing the same position for [`Self::index`], but reads no data.
    pub fn take_reader(&mut self) -> Option<Self> {
        let rx = self.rx.take()?;
        if !self.path.is_empty() {
            self.state
                .index
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(&self.path);
        }
        Some(Self {
            rx: Some(rx),
            subscribed: true,
            path: Arc::clone(&self.path),
            dropped: Arc::clone(&self.dropped),
            task: Arc::clone(&self.task),
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
        let n = buf.filled().len();
        ready!(rx.poll_read(cx, buf))?;
        trace!(buf = ?buf.filled(), "read buffer");
        if buf.filled().len() == n {
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
    done: watch::Sender<Option<std::io::Result<()>>>,
}

impl IngressState {
    /// Returns whether an empty frame closed the `path`
    fn is_closed(&self, path: &[usize]) -> bool {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        matches!(
            index.subs.get(path).and_then(|sub| sub.tx.as_ref()),
            Some(TxSlot::Closed)
        )
    }
}

/// Outgoing framed stream
#[derive(Clone)]
pub struct Outgoing {
    tx: PollSender<(Bytes, Bytes)>,
    path: Arc<[usize]>,
    path_buf: Bytes,
    done: watch::Receiver<Option<std::io::Result<()>>>,
}

impl Outgoing {
    /// Creates a new [Outgoing] given an [`AsyncWrite`].
    pub fn new<T>(mut tx: T) -> Self
    where
        T: AsyncWrite + Unpin + Send + 'static,
    {
        let span = Span::current();
        let (tx_tx, tx_rx) = mpsc::channel(128);
        let (done_tx, done_rx) = watch::channel(None);
        tokio::spawn(
            async move {
                let res = egress(&mut tx, tx_rx).await;
                if let Err(err) = &res {
                    error!(?err, "egress failed");
                } else {
                    debug!("egress successfully complete");
                }
                done_tx.send_replace(Some(res));
            }
            .instrument(span.clone()),
        );
        Self {
            tx: PollSender::new(tx_tx),
            path: Arc::from([]),
            path_buf: Bytes::from_static(&[0]),
            done: done_rx,
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
            done: self.done.clone(),
        })
    }

    /// Returns a future, which resolves to the result of egress once it is complete.
    pub fn finished(&self) -> impl Future<Output = std::io::Result<()>> + Send + 'static {
        finished(self.done.clone())
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
                match subs.route(&path, &mut permit) {
                    Route::Tx(tx) => {
                        if n == 0 {
                            trace!(?path, "closing path");
                            *tx = TxSlot::Closed;
                            continue 'frames;
                        }
                        let TxSlot::Open(tx) = tx else {
                            return Err(error_closed(&path));
                        };
                        break tx.clone();
                    }
                    Route::NotFound | Route::Released if n == 0 => {
                        trace!(?path, "discarding empty frame");
                        continue 'frames;
                    }
                    Route::NotFound => return Err(error_not_found(&path)),
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

/// Returns the error for data received on a closed `path`
fn error_closed(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("received data on closed path `{path:?}`"),
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

/// Waits for the result of ingress or egress
async fn finished(mut done: watch::Receiver<Option<std::io::Result<()>>>) -> std::io::Result<()> {
    let Ok(res) = done.wait_for(Option::is_some).await else {
        return Err(std::io::Error::other("I/O task stopped without a result"));
    };
    match &*res {
        Some(Err(err)) => Err(copy_io_error(err)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use std::io::ErrorKind;

    use futures::FutureExt as _;
    use futures::future::BoxFuture;
    use tokio::io::{BufReader, DuplexStream};

    use super::*;

    const FRAME: [u8; 4] = [0x01, 0x00, 0x01, 0x2a];

    fn incoming() -> (
        DuplexStream,
        Incoming,
        BoxFuture<'static, std::io::Result<()>>,
    ) {
        incoming_paths([[Some(0)]])
    }

    fn incoming_paths<P: AsRef<[Option<usize>]>>(
        paths: impl IntoIterator<Item = P>,
    ) -> (
        DuplexStream,
        Incoming,
        BoxFuture<'static, std::io::Result<()>>,
    ) {
        let (tx, rx) = tokio::io::duplex(16);
        let rx = Incoming::new(BufReader::new(rx), paths);
        let done = rx.finished().boxed();
        (tx, rx, done)
    }

    async fn done(rx: BoxFuture<'static, std::io::Result<()>>) -> std::io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("ingress timed out")
    }

    async fn failed(rx: BoxFuture<'static, std::io::Result<()>>) -> ErrorKind {
        done(rx).await.unwrap_err().kind()
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_unsubscribed_paths() {
        let (_tx, rx, ..) = incoming_paths([&[Some(0)][..], &[Some(1)], &[Some(1), Some(0)]]);
        rx.index(&[0]).unwrap();
        for path in [&[0][..], &[2], &[1, 1]] {
            let Err(err) = rx.index(path) else {
                panic!("indexing `{path:?}` should fail");
            };
            assert_eq!(err.kind(), ErrorKind::NotFound);
        }
        let (_tx, rx, ..) = incoming_paths([[Some(0), Some(0)]]);
        let mut sub = rx.index(&[0]).unwrap();
        sub.index(&[0]).unwrap();
        let err = sub.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_read_to_end_fails_on_ingress_failure() {
        let (mut tx, mut rx, ..) = incoming();
        tx.write_all(&[0x00, 0x01, 0x2a, 0x01]).await.unwrap();
        drop(tx);
        let mut buf = Vec::default();
        let err = rx.read_to_end(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
        assert_eq!(buf, [0x2a]);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_aborts_without_finished() {
        for finished_first in [true, false] {
            let (mut tx, rx, done_rx) = incoming();
            if finished_first {
                drop(done_rx);
                drop(rx);
            } else {
                drop(rx);
                tokio::task::yield_now().await;
                drop(done_rx);
            }
            tokio::task::yield_now().await;
            let err = tx.write_all(&FRAME).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BrokenPipe);
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_drop_waits_for_eof() {
        let (mut tx, rx, mut done_rx) = incoming();
        drop(rx);
        tokio::task::yield_now().await;
        assert!((&mut done_rx).now_or_never().is_none());
        tx.write_all(&[0x00, 0x00]).await.unwrap();
        tokio::task::yield_now().await;
        assert!((&mut done_rx).now_or_never().is_none());
        drop(tx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_eof_waits_for_drop() {
        let (tx, mut rx, mut done_rx) = incoming();
        drop(tx);
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        assert!((&mut done_rx).now_or_never().is_none());
        drop(rx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_empty_frame_closes_path() {
        let (mut tx, mut rx, done_rx) = incoming();
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
        let (mut tx, rx, done_rx) = incoming();
        let finished = rx.finished();
        drop(rx);
        tx.write_all(&FRAME).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
        assert_eq!(finished.await.unwrap_err().kind(), ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming();
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming();
        let sub = rx.index(&[0]).unwrap();
        tx.write_all(&FRAME).await.unwrap();
        tokio::task::yield_now().await;
        drop(sub);
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (tx, rx, done_rx) = incoming();
        drop(crate::BufferedIncoming {
            buffer: BytesMut::from(&FRAME[..]),
            inner: rx,
        });
        drop(tx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming();
        tx.write_all(&FRAME).await.unwrap();
        drop(tx);
        tokio::task::yield_now().await;
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming();
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::NotFound);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_finished() {
        let (tx, rx) = tokio::io::duplex(16);
        drop(rx);
        let mut tx = Outgoing::new(tx);
        let finished = tx.finished();
        tx.write_all(&[0x2a]).await.unwrap();
        drop(tx);
        assert_eq!(finished.await.unwrap_err().kind(), ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_declared_paths() {
        let (mut tx, rx, done_rx) = incoming_paths([
            &[Some(0)][..],
            &[Some(0)],
            &[Some(0xffff_ffff)],
            &[Some(1), None, Some(0)],
            &[Some(1), None],
        ]);
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x01, 0x2b])
            .await
            .unwrap();
        tx.write_all(&[0x02, 0x01, 0x05, 0x01, 0x2c]).await.unwrap();
        tx.write_all(&[0x03, 0x01, 0x07, 0x00, 0x01, 0x2d])
            .await
            .unwrap();
        drop(tx);
        for (path, data) in [
            (&[0][..], 0x2a),
            (&[0xffff_ffff], 0x2b),
            (&[1, 5], 0x2c),
            (&[1, 7, 0], 0x2d),
        ] {
            let mut buf = Vec::default();
            rx.index(path).unwrap().read_to_end(&mut buf).await.unwrap();
            assert_eq!(buf, [data]);
        }
        drop(rx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_take_reader() {
        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)]]);
        tx.write_all(&[0x00, 0x01, 0x2b]).await.unwrap();
        tx.write_all(&FRAME).await.unwrap();
        drop(tx);
        let mut reader = rx.take_reader().unwrap();
        assert!(rx.take_reader().is_none());
        let mut buf = Vec::default();
        let mut sub = rx.index(&[0]).unwrap();
        sub.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        buf.clear();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2b]);
        drop(sub);
        drop(rx);
        drop(reader);
        done(done_rx).await.unwrap();

        let (mut tx, rx, done_rx) = incoming_paths([[None]]);
        let mut element = rx.index(&[0]).unwrap();
        let mut reader = element.take_reader().unwrap();
        drop(element);
        tx.write_all(&FRAME).await.unwrap();
        drop(tx);
        let mut buf = Vec::default();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        assert_eq!(rx.state.index.lock().unwrap().subs.len(), 2);
        drop(reader);
        assert_eq!(rx.state.index.lock().unwrap().subs.len(), 1);
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
            (
                &[0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x2a],
                ErrorKind::InvalidData,
            ),
            (&[0x01, 0x01, 0x01, 0x2a], ErrorKind::NotFound),
            (&[0x03, 0x02, 0x03, 0x00, 0x01, 0x2a], ErrorKind::NotFound),
        ] {
            let (mut tx, _rx, done_rx) =
                incoming_paths([[Some(0)].as_slice(), &[Some(1), Some(0)], &[Some(2), None]]);
            tx.write_all(frames).await.unwrap();
            drop(tx);
            assert_eq!(failed(done_rx).await, kind, "{frames:02x?}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_discards_empty_frames_on_closed_paths() {
        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)], [Some(1)]]);
        let mut sub = rx.index(&[0]).unwrap();
        let mut other = rx.index(&[1]).unwrap();
        tx.write_all(&[0x00, 0x00, 0x00, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00])
            .await
            .unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        let mut buf = [0; 1];
        other.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        assert_eq!(rx.read_to_end(&mut Vec::default()).await.unwrap(), 0);
        assert_eq!(sub.read_to_end(&mut Vec::default()).await.unwrap(), 0);
        tx.write_all(&FRAME).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_discards_empty_frames_on_undeclared_paths() {
        let (mut tx, rx, done_rx) =
            incoming_paths([[Some(0)].as_slice(), &[Some(1), Some(0)], &[Some(2), None]]);
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&[0x01, 0x01, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x03, 0x00]).await.unwrap();
        tx.write_all(&[0x03, 0x02, 0x03, 0x00, 0x00]).await.unwrap();
        tx.write_all(&FRAME).await.unwrap();
        let mut buf = [0; 1];
        sub.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        {
            let index = rx.state.index.lock().unwrap();
            assert_eq!(index.subs.len(), 3, "{index:?}");
            assert!(index.next.is_empty(), "{index:?}");
            assert_eq!(
                index.unclaimed.available_permits(),
                MAX_UNCLAIMED_ELEMENTS,
                "{index:?}"
            );
        }
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::NotFound);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_discards_empty_frames_on_released_elements() {
        let (mut tx, rx, done_rx) = incoming_paths([[None]]);
        drop(rx.index(&[0]).unwrap());
        let mut sub = rx.index(&[1]).unwrap();
        tx.write_all(&[0x01, 0x00, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x00, 0x00]).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        let mut buf = [0; 1];
        sub.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0x2a]);
        assert!(
            !rx.state
                .index
                .lock()
                .unwrap()
                .subs
                .contains_key([0].as_slice())
        );
        tx.write_all(&FRAME).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_wildcard_elements() {
        let (mut tx, rx, done_rx) = incoming_paths([[None]]);
        tx.write_all(&FRAME).await.unwrap();
        tx.write_all(&[0x01, 0x03, 0x01, 0x2b]).await.unwrap();
        tx.write_all(&[0x01, 0x01, 0x01, 0x2a]).await.unwrap();
        tokio::task::yield_now().await;
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
        tx.write_all(&[0x01, 0x00, 0x00]).await.unwrap();
        tx.write_all(&FRAME).await.unwrap();
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
        let (mut tx, rx, done_rx) = incoming_paths([[None, Some(0), None]]);
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
        let (mut tx, rx, done_rx) = incoming_paths([[None, None]]);
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
