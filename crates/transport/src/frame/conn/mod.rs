use core::fmt::{Debug, Display};
use core::iter::zip;
use core::mem;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker, ready};

use std::collections::{BTreeMap, btree_map};
use std::io::IoSlice;
use std::sync::{Arc, PoisonError};
use std::task::Wake;

use bytes::{Buf as _, Bytes, BytesMut};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::codec::{Decoder, Encoder as _};
use tracing::{debug, error, instrument, trace};
use wasm_tokio::Leb128Encoder;

use crate::frame::FrameHeaderDecoder;
use crate::{Index, copy_io_error, error_unprocessed};

mod client;
mod server;

pub use client::*;
pub use server::*;

/// Maximum length of the data of a frame
const MAX_FRAME_DATA: usize = u32::MAX as usize;

/// Error returned by [`HeaderDecoder`](crate::frame::HeaderDecoder)
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

impl From<std::io::Error> for HeaderReadError {
    fn from(err: std::io::Error) -> Self {
        Self::IO(err)
    }
}

/// wRPC invocation header
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Header {
    /// Instance name of the function being called
    pub instance: String,

    /// Name of the function being called
    pub name: String,
}

/// Frame being received
#[derive(Debug)]
struct Frame {
    path: Box<[usize]>,
    /// Length of the data left to receive
    remaining: usize,
}

/// Wakers of the handles waiting for ingress. The stream is polled with a [Waker] of this set,
/// which wakes all of them.
#[derive(Debug, Default)]
struct Readers {
    /// Waker of each live [Incoming] handle reading a declared path, keyed by it
    handles: std::sync::Mutex<BTreeMap<Box<[usize]>, Waker>>,
    /// Wakers of the [Closed] futures
    closed: std::sync::Mutex<Vec<Waker>>,
}

impl Readers {
    fn wake_all(&self) {
        let handles: Vec<Waker> = lock(&self.handles).values().cloned().collect();
        let closed = mem::take(&mut *lock(&self.closed));
        for waker in handles.into_iter().chain(closed) {
            waker.wake();
        }
    }
}

impl Wake for Readers {
    fn wake(self: Arc<Self>) {
        self.wake_all();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_all();
    }
}

/// State of an invocation shared by ingress and egress
#[derive(Debug, Default)]
struct Shared {
    /// Whether the invocation failed
    failed: AtomicBool,
    /// Error the invocation failed with
    error: std::sync::Mutex<Option<std::io::Error>>,
    /// Number of [Outgoing] handles
    outgoing: AtomicUsize,
    readers: Arc<Readers>,
    /// Wakers of the handles waiting for egress and of [Closed] waiting for the [Outgoing]
    /// handles to be dropped. The stream is polled with a [Waker] of this set, which wakes all
    /// of them.
    wakers: std::sync::Mutex<Vec<Waker>>,
}

impl Shared {
    /// Fails the invocation with `err`, unless it failed already, waking all handles
    fn fail(&self, err: &std::io::Error) {
        let mut error = lock(&self.error);
        if error.is_some() {
            return;
        }
        error!(?err, "invocation failed");
        *error = Some(copy_io_error(err));
        self.failed.store(true, Ordering::Release);
        drop(error);
        self.readers.wake_all();
        self.wake_all();
    }

    /// Returns the error the invocation failed with
    fn error(&self) -> std::io::Error {
        lock(&self.error).as_ref().map_or_else(
            || std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "invocation failed"),
            copy_io_error,
        )
    }

    fn wake_all(&self) {
        let wakers = mem::take(&mut *lock(&self.wakers));
        for waker in wakers {
            waker.wake();
        }
    }
}

impl Wake for Shared {
    fn wake(self: Arc<Self>) {
        self.wake_all();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_all();
    }
}

/// Ingress of an invocation
struct Ingress<I> {
    rx: I,
    /// Declared paths
    paths: Box<[Box<[Option<usize>]>]>,
    /// Decoder of the header of the frame being received
    header: FrameHeaderDecoder,
    /// Bytes handed to the frame header decoder
    buf: BytesMut,
    /// Frame being received, once its header is complete
    frame: Option<Frame>,
    /// Number of [Incoming] handles
    handles: usize,
    /// Whether ingress ended
    ended: bool,
    shared: Arc<Shared>,
}

impl<I> Ingress<I> {
    /// Fails the invocation with `err`
    fn fail(&self, err: std::io::Error) -> std::io::Error {
        self.shared.fail(&err);
        err
    }

    /// Routes the frame received on `path` with `n` bytes of data
    fn route(&mut self, path: Vec<usize>, n: usize) -> std::io::Result<()> {
        trace!(?path, n, "received frame header");
        if resolve(&self.paths, &path) != Some(true) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("received frame on undeclared path `{path:?}`"),
            ));
        }
        let path = Box::from(path);
        let waker = lock(&self.shared.readers.handles).get(&path).cloned();
        if waker.is_none() && self.handles == 0 {
            return Err(error_unprocessed());
        }
        if waker.is_none() {
            trace!("holding frame until the path is read");
        }
        self.frame = Some(Frame { path, remaining: n });
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}

impl<I> Ingress<I>
where
    I: AsyncBufRead + Unpin,
{
    /// Receives the header of the next frame, routing the frame, or ends ingress at the end of the
    /// stream
    fn poll_frame(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let max = self.header.max_len();
        let header = ready!(poll_decode(
            &mut self.rx,
            &mut self.header,
            max,
            &mut self.buf,
            cx
        ))?;
        let Some((path, n)) = header else {
            if !lock(&self.shared.readers.handles).is_empty() {
                return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
            }
            debug!("ingress complete");
            self.ended = true;
            self.shared.readers.wake_all();
            return Poll::Ready(Ok(()));
        };
        self.route(path, n)?;
        Poll::Ready(Ok(()))
    }

    /// Polls `poll` with the [Waker] of the handles waiting for ingress, failing the invocation
    /// on error
    fn poll_ingress<R>(
        &mut self,
        poll: impl FnOnce(&mut Self, &mut Context<'_>) -> Poll<std::io::Result<R>>,
    ) -> Poll<std::io::Result<R>> {
        let waker = Waker::from(Arc::clone(&self.shared.readers));
        poll(self, &mut Context::from_waker(&waker)).map_err(|err| self.fail(err))
    }

    /// Receives frame headers until a frame with data is routed, waking the handle reading it,
    /// or the stream ends
    fn poll_pull(&mut self) -> Poll<std::io::Result<()>> {
        while !self.ended && self.frame.is_none() {
            ready!(self.poll_ingress(Ingress::poll_frame))?;
        }
        Poll::Ready(Ok(()))
    }
}

/// Egress of an invocation
struct Egress<O: AsyncWrite + Unpin> {
    tx: O,
    /// Bytes of the header of the frame being written, which the stream did not accept yet
    buf: BytesMut,
    /// Path of the frame being written and the length of its data left to write, once the
    /// stream accepted part of it
    frame: Option<(Bytes, usize)>,
    shared: Arc<Shared>,
}

impl<O: AsyncWrite + Unpin> Drop for Egress<O> {
    fn drop(&mut self) {
        if self.shared.failed.load(Ordering::Acquire) {
            return;
        }
        let mut cx = Context::from_waker(Waker::noop());
        if let Poll::Ready(Ok(())) = self.poll_drain(&mut cx)
            && self.frame.is_none()
        {
            _ = Pin::new(&mut self.tx).poll_shutdown(&mut cx);
        }
    }
}

impl<O> Egress<O>
where
    O: AsyncWrite + Unpin,
{
    /// Fails the invocation with `err`
    fn fail(&self, err: std::io::Error) -> std::io::Error {
        self.shared.fail(&err);
        err
    }

    /// Writes the buffered bytes on behalf of a handle, failing the invocation on error
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.shared.failed.load(Ordering::Acquire) {
            return Poll::Ready(Err(self.shared.error()));
        }
        register(&mut lock(&self.shared.wakers), cx.waker());
        let waker = Waker::from(Arc::clone(&self.shared));
        let mut cx = Context::from_waker(&waker);
        while !self.buf.is_empty() {
            match Pin::new(&mut self.tx).poll_write(&mut cx, &self.buf) {
                Poll::Ready(Ok(0)) => {
                    let err = std::io::ErrorKind::WriteZero.into();
                    return Poll::Ready(Err(self.fail(err)));
                }
                Poll::Ready(Ok(n)) => self.buf.advance(n),
                Poll::Ready(Err(err)) => return Poll::Ready(Err(self.fail(err))),
                Poll::Pending => return Poll::Pending,
            }
        }
        if let Some((.., 0)) = self.frame {
            self.frame = None;
            self.shared.wake_all();
        }
        Poll::Ready(Ok(()))
    }

    /// Writes the buffered bytes, then the frame with `data` on the path encoded as `path_buf`,
    /// or continues the frame being written on it, on behalf of a handle, returning the number
    /// of bytes of `data` the stream accepted. Once the stream accepted part of a frame, the
    /// frames of other paths wait for it to be written. The header of an empty frame the stream
    /// does not accept at once is buffered, so that closing a path never waits for the stream.
    fn poll_write(
        &mut self,
        path_buf: &Bytes,
        data: &[u8],
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        ready!(self.poll_drain(cx))?;
        let waker = Waker::from(Arc::clone(&self.shared));
        let mut cx = Context::from_waker(&waker);
        if let Some((path, remaining)) = &mut self.frame {
            if *path != *path_buf {
                trace!("waiting for the frame of another path to be written");
                return Poll::Pending;
            }
            if data.is_empty() {
                let err = std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "frame being written is incomplete",
                );
                return Poll::Ready(Err(self.fail(err)));
            }
            let data = &data[..data.len().min(*remaining)];
            trace!(?data, "writing outgoing frame data");
            let n = match ready!(Pin::new(&mut self.tx).poll_write(&mut cx, data)) {
                Ok(0) => {
                    let err = std::io::ErrorKind::WriteZero.into();
                    return Poll::Ready(Err(self.fail(err)));
                }
                Ok(n) => n,
                Err(err) => return Poll::Ready(Err(self.fail(err))),
            };
            *remaining = remaining.saturating_sub(n);
            if *remaining == 0 {
                self.frame = None;
                self.shared.wake_all();
            }
            return Poll::Ready(Ok(n));
        }
        let n = u32::try_from(data.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        let mut len_buf = BytesMut::with_capacity(5);
        Leb128Encoder.encode(n, &mut len_buf)?;
        trace!(?path_buf, ?data, "writing outgoing frame");
        let header = path_buf.len().saturating_add(len_buf.len());
        let bufs = [&path_buf[..], &len_buf[..], data];
        let mut written = 0;
        loop {
            let mut skip = written;
            let slices = bufs.map(|buf| {
                let k = skip.min(buf.len());
                skip = skip.saturating_sub(k);
                IoSlice::new(&buf[k..])
            });
            match Pin::new(&mut self.tx).poll_write_vectored(&mut cx, &slices) {
                Poll::Ready(Ok(0)) => {
                    let err = std::io::ErrorKind::WriteZero.into();
                    return Poll::Ready(Err(self.fail(err)));
                }
                Poll::Ready(Ok(n)) => {
                    written = written.saturating_add(n);
                    let Some(accepted) = written.checked_sub(header) else {
                        continue;
                    };
                    let remaining = data.len().saturating_sub(accepted);
                    if remaining == 0 {
                        self.frame = None;
                        return Poll::Ready(Ok(accepted));
                    }
                    self.frame = Some((path_buf.clone(), remaining));
                    if accepted > 0 {
                        return Poll::Ready(Ok(accepted));
                    }
                }
                Poll::Ready(Err(err)) => return Poll::Ready(Err(self.fail(err))),
                Poll::Pending if written > 0 || data.is_empty() => {
                    let k = written.min(path_buf.len());
                    self.buf.extend_from_slice(&path_buf[k..]);
                    let k = written.saturating_sub(k).min(len_buf.len());
                    self.buf.extend_from_slice(&len_buf[k..]);
                    self.frame = Some((path_buf.clone(), data.len()));
                    if data.is_empty() {
                        return Poll::Ready(Ok(0));
                    }
                    return Poll::Pending;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Decodes an item from `rx` using `dec`, which consumes all bytes it is given up to the end of
/// the item, handing it at most `max` buffered bytes at a time copied to `src` and consuming from
/// `rx` exactly the bytes it took. Returns [None] at the end of the stream, unless `dec` fails on
/// it.
fn poll_decode<D: Decoder>(
    rx: &mut (impl AsyncBufRead + Unpin),
    dec: &mut D,
    max: usize,
    src: &mut BytesMut,
    cx: &mut Context<'_>,
) -> Poll<Result<Option<D::Item>, D::Error>> {
    loop {
        let buf = ready!(Pin::new(&mut *rx).poll_fill_buf(cx))?;
        src.clear();
        if buf.is_empty() {
            return Poll::Ready(dec.decode_eof(src));
        }
        src.extend_from_slice(&buf[..buf.len().min(max)]);
        let n = src.len();
        let item = dec.decode(src)?;
        Pin::new(&mut *rx).consume(n.saturating_sub(src.len()));
        if item.is_some() {
            return Poll::Ready(Ok(item));
        }
    }
}

/// Resolves `path` to the declared path matching it, returning whether it is declared, or
/// [None] if it is not a prefix of a declared path
fn resolve(paths: &[Box<[Option<usize>]>], path: &[usize]) -> Option<bool> {
    if path.is_empty() {
        return Some(true);
    }
    let mut prefix = false;
    for declared in paths {
        if declared.len() >= path.len()
            && zip(declared, path)
                .all(|(declared, i)| declared.is_none_or(|declared| declared == *i))
        {
            if declared.len() == path.len() {
                return Some(true);
            }
            prefix = true;
        }
    }
    prefix.then_some(false)
}

/// Joins `path` relative to `base`
fn join_path(base: &[usize], path: &[usize]) -> std::io::Result<Vec<usize>> {
    if path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path cannot be empty",
        ));
    }
    Ok([base, path].concat())
}

/// Registers `waker` in `wakers`, unless a waker, which would wake the same task, is registered
fn register(wakers: &mut Vec<Waker>, waker: &Waker) {
    if !wakers.iter().any(|w| w.will_wake(waker)) {
        wakers.push(waker.clone());
    }
}

/// Locks `state`, recovering it if a thread panicked while holding the lock
fn lock<T>(state: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Multiplexes the [`AsyncWrite`] and [`AsyncBufRead`] of an invocation declaring the async
/// `paths`, returning the [Outgoing] and [Incoming] handles of the root path.
///
/// The streams are polled by the handles, nothing is spawned: data is received only while a
/// handle reads or the future returned by [`Incoming::closed`] is polled, and sent only while a
/// handle writes, flushes or shuts down. The header of a frame the stream does not accept at
/// once is buffered and written on the next of those, so a handle must be shut down before it
/// is dropped for all of its data to be sent. The writer is shut down and dropped once all
/// [Outgoing] handles are, which ends transmit, unless the invocation failed.
/// A frame is received as a whole before the next one, so the handle of its path must read it
/// for any other handle to make progress, i.e. all handles of an invocation must be read
/// concurrently.
/// A frame received on a path, which no handle reads, is held until a handle reads it, so a
/// frame received on a path after it was closed stalls ingress like a frame received on a path,
/// which was not indexed yet. Any frame received on a path, which is not declared, fails the
/// invocation, as does a frame received on a path, which no handle reads, once all handles are
/// dropped, dropping a handle before it read the end of the data of its path, and the stream
/// ending while a frame is being received or a handle still reads a path, which was not closed.
/// If the invocation fails, every handle fails and the writer is dropped without being shut
/// down, so the peer observes the end of transmit before the root path is closed and fails the
/// invocation too.
///
/// `paths` must not contain both a wildcard and an index at the same position, so paths of
/// different variant cases, which are not indexed, must be merged by replacing such indices
/// by wildcards.
pub(crate) fn mux<I, O, P>(
    tx: O,
    rx: I,
    paths: impl IntoIterator<Item = P>,
) -> (Outgoing<O>, Incoming<I>)
where
    O: AsyncWrite + Unpin,
    P: AsRef<[Option<usize>]>,
{
    let paths: Box<[Box<[Option<usize>]>]> = paths
        .into_iter()
        .map(|path| Box::<[Option<usize>]>::from(path.as_ref()))
        .filter(|path| !path.is_empty())
        .collect();
    let depth = paths.iter().map(|path| path.len()).max().unwrap_or(0);
    let depth = u32::try_from(depth).unwrap_or(u32::MAX);
    let shared = Arc::new(Shared {
        outgoing: AtomicUsize::new(1),
        ..Shared::default()
    });
    lock(&shared.readers.handles).insert(Box::default(), Waker::noop().clone());
    (
        Outgoing {
            path: Box::default(),
            path_buf: Bytes::from_static(&[0]),
            closed: false,
            egress: Arc::new(std::sync::Mutex::new(Egress {
                tx,
                buf: BytesMut::default(),
                frame: Option::default(),
                shared: Arc::clone(&shared),
            })),
        },
        Incoming {
            subscribed: true,
            closed: false,
            path: Box::default(),
            buffer: BytesMut::default(),
            ingress: Arc::new(std::sync::Mutex::new(Ingress {
                rx,
                paths,
                header: FrameHeaderDecoder::new(depth, u32::MAX),
                buf: BytesMut::default(),
                frame: Option::default(),
                handles: 1,
                ended: false,
                shared,
            })),
        },
    )
}

/// Incoming framed stream
pub struct Incoming<T> {
    /// Whether this handle reads the data of its path
    subscribed: bool,
    /// Whether this handle read the end of the data of its path
    closed: bool,
    path: Box<[usize]>,
    /// Bytes returned by [`Self::unread`], which are read before the data of the path
    buffer: BytesMut,
    ingress: Arc<std::sync::Mutex<Ingress<T>>>,
}

impl<T> Drop for Incoming<T> {
    fn drop(&mut self) {
        let mut ingress = lock(&self.ingress);
        ingress.handles = ingress.handles.saturating_sub(1);
        if self.subscribed && !self.closed {
            lock(&ingress.shared.readers.handles).remove(&self.path);
            trace!(path = ?self.path, "dropping handle of an open path");
            ingress.fail(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("`{:?}` dropped before it was closed", self.path),
            ));
        }
        if !self.buffer.is_empty() || (ingress.handles == 0 && ingress.frame.is_some()) {
            trace!(path = ?self.path, "dropping handle with unprocessed data");
            ingress.fail(error_unprocessed());
        }
    }
}

impl<T> Incoming<T> {
    /// Index the incoming stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it. A declared path may be indexed once, the handle
    /// of a path, which is not declared, only serves to index the paths nested in it.
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        let path = join_path(&self.path, path)?;
        let path = Box::from(path);
        let mut ingress = lock(&self.ingress);
        let Some(declared) = resolve(&ingress.paths, &path) else {
            return Err(error_not_found(&path));
        };
        let mut subscribed = false;
        if declared {
            match lock(&ingress.shared.readers.handles).entry(path.clone()) {
                btree_map::Entry::Occupied(..) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("`{path:?}` already indexed"),
                    ));
                }
                btree_map::Entry::Vacant(entry) => {
                    entry.insert(Waker::noop().clone());
                    subscribed = true;
                }
            }
        }
        ingress.handles = ingress.handles.saturating_add(1);
        Ok(Self {
            subscribed,
            closed: false,
            path,
            buffer: BytesMut::default(),
            ingress: Arc::clone(&self.ingress),
        })
    }

    /// Returns a future, which resolves to the result of ingress once the invocation ends, that
    /// is once ingress ends and all [Outgoing] handles are dropped.
    /// Ingress is received until the peer ends transmit.
    #[must_use]
    pub fn closed(&self) -> Closed<T> {
        Closed(Arc::clone(&self.ingress))
    }

    /// Returns `bytes` read from this handle to it, so that they are read again before the rest
    /// of the data of the path
    pub fn unread(&mut self, mut bytes: BytesMut) {
        bytes.unsplit(mem::take(&mut self.buffer));
        self.buffer = bytes;
    }
}

impl<T> Index for Incoming<T> {
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        Self::index(self, path)
    }
}

impl<T> AsyncRead for Incoming<T>
where
    T: AsyncBufRead + Unpin,
{
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let this = self.get_mut();
        if !this.subscribed {
            return Poll::Ready(Err(error_not_found(&this.path)));
        }
        if !this.buffer.is_empty() {
            let n = buf.remaining().min(this.buffer.len());
            buf.put_slice(&this.buffer.split_to(n));
            return Poll::Ready(Ok(()));
        }
        if this.closed {
            return Poll::Ready(Ok(()));
        }
        trace!("reading");
        let mut ingress = lock(&this.ingress);
        let ingress = &mut *ingress;
        loop {
            if ingress.shared.failed.load(Ordering::Acquire) {
                return Poll::Ready(Err(ingress.shared.error()));
            }
            if let Some(waker) = lock(&ingress.shared.readers.handles).get_mut(&this.path)
                && !waker.will_wake(cx.waker())
            {
                *waker = cx.waker().clone();
            }
            let remaining = match &ingress.frame {
                Some(frame) if *frame.path == *this.path && frame.remaining == 0 => {
                    trace!("closing path");
                    ingress.frame = None;
                    lock(&ingress.shared.readers.handles).remove(&this.path);
                    this.closed = true;
                    _ = ingress.poll_pull();
                    return Poll::Ready(Ok(()));
                }
                Some(frame) if *frame.path == *this.path => frame.remaining,
                Some(frame) => {
                    trace!(path = ?frame.path, "waiting for the frame of another path to be read");
                    return Poll::Pending;
                }
                None => {
                    ready!(ingress.poll_pull())?;
                    continue;
                }
            };
            let n = ready!(ingress.poll_ingress(|ingress, cx| {
                let data = ready!(Pin::new(&mut ingress.rx).poll_fill_buf(cx))?;
                if data.is_empty() {
                    return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
                }
                let n = buf.remaining().min(data.len()).min(remaining);
                buf.put_slice(&data[..n]);
                Pin::new(&mut ingress.rx).consume(n);
                Poll::Ready(Ok(n))
            }))?;
            trace!(buf = ?buf.filled(), "read buffer");
            if let Some(frame) = &mut ingress.frame {
                frame.remaining = remaining.saturating_sub(n);
                if frame.remaining == 0 {
                    ingress.frame = None;
                    _ = ingress.poll_pull();
                }
            }
            return Poll::Ready(Ok(()));
        }
    }
}

/// Future returned by [`Incoming::closed`]
pub struct Closed<T>(Arc<std::sync::Mutex<Ingress<T>>>);

impl<T> Debug for Closed<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Closed").finish_non_exhaustive()
    }
}

impl<T> Future for Closed<T>
where
    T: AsyncBufRead + Unpin,
{
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut ingress = lock(&self.0);
        loop {
            if ingress.shared.failed.load(Ordering::Acquire) {
                return Poll::Ready(Err(ingress.shared.error()));
            }
            if ingress.ended {
                if ingress.shared.outgoing.load(Ordering::Acquire) == 0 {
                    return Poll::Ready(Ok(()));
                }
                register(&mut lock(&ingress.shared.wakers), cx.waker());
                if ingress.shared.outgoing.load(Ordering::Acquire) == 0 {
                    return Poll::Ready(Ok(()));
                }
                return Poll::Pending;
            }
            register(&mut lock(&ingress.shared.readers.closed), cx.waker());
            if ingress.frame.is_some() {
                return Poll::Pending;
            }
            ready!(ingress.poll_pull())?;
        }
    }
}

/// Outgoing framed stream
pub struct Outgoing<T: AsyncWrite + Unpin> {
    path: Box<[usize]>,
    path_buf: Bytes,
    /// Whether the stream was shut down
    closed: bool,
    egress: Arc<std::sync::Mutex<Egress<T>>>,
}

impl<T: AsyncWrite + Unpin> Drop for Outgoing<T> {
    fn drop(&mut self) {
        let egress = lock(&self.egress);
        if let Some((path, remaining)) = &egress.frame
            && *path == self.path_buf
            && *remaining > 0
        {
            trace!("dropping handle with an incomplete frame");
            egress.fail(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "frame being written is incomplete",
            ));
        }
        if egress.shared.outgoing.fetch_sub(1, Ordering::AcqRel) == 1 {
            trace!("all handles dropped");
            egress.shared.wake_all();
        }
    }
}

impl<T: AsyncWrite + Unpin> Outgoing<T> {
    /// Index the outgoing stream using a structural `path`, returning a handle that writes
    /// to the multiplexed sub-stream addressed by it.
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        let path = join_path(&self.path, path)?;
        let mut buf = BytesMut::with_capacity(path.len().saturating_add(5));
        let n = u32::try_from(path.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding path length");
        Leb128Encoder.encode(n, &mut buf)?;
        for p in &path {
            let p = u32::try_from(*p)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(p, "encoding path element");
            Leb128Encoder.encode(p, &mut buf)?;
        }
        let egress = lock(&self.egress);
        egress.shared.outgoing.fetch_add(1, Ordering::AcqRel);
        Ok(Self {
            path: path.into(),
            path_buf: buf.freeze(),
            closed: false,
            egress: Arc::clone(&self.egress),
        })
    }
}

impl<T: AsyncWrite + Unpin> Outgoing<T> {
    /// Returns a handle writing the data of the same path, which does not close it
    pub(crate) fn nested(&self) -> Self {
        let egress = lock(&self.egress);
        egress.shared.outgoing.fetch_add(1, Ordering::AcqRel);
        Self {
            path: self.path.clone(),
            path_buf: self.path_buf.clone(),
            closed: true,
            egress: Arc::clone(&self.egress),
        }
    }
}

impl<T: AsyncWrite + Unpin> Index for Outgoing<T> {
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        Self::index(self, path)
    }
}

/// Writes to the stream as frames of at most `u32::MAX` bytes of data. The stream may accept
/// a frame partially, in which case the data written next on the path continues it, so a
/// handle must not be shut down or dropped before it wrote all the data it was asked to.
/// Shutting the stream down closes it by writing an empty frame, so no more data may be written
/// afterwards.
impl<T> AsyncWrite for Outgoing<T>
where
    T: AsyncWrite + Unpin,
{
    #[instrument(level = "trace", skip_all, fields(path = ?self.path, buf = format!("{buf:02x?}")), ret(level = "trace"))]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let buf = &buf[..buf.len().min(MAX_FRAME_DATA)];
        let this = self.get_mut();
        let mut egress = lock(&this.egress);
        egress.poll_write(&this.path_buf, buf, cx)
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut egress = lock(&this.egress);
        let egress = &mut *egress;
        ready!(egress.poll_drain(cx))?;
        let waker = Waker::from(Arc::clone(&egress.shared));
        match Pin::new(&mut egress.tx).poll_flush(&mut Context::from_waker(&waker)) {
            Poll::Ready(Err(err)) => Poll::Ready(Err(egress.fail(err))),
            res => res,
        }
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if !this.closed {
            trace!("closing outgoing path");
            let mut egress = lock(&this.egress);
            match egress.poll_write(&this.path_buf, &[], cx) {
                Poll::Ready(Ok(..)) => this.closed = true,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Pin::new(this).poll_flush(cx)
    }
}

/// Returns the error for a `path`, which is not subscribed to
fn error_not_found(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("`{path:?}` subscription not found"),
    )
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use std::io::ErrorKind;

    use futures::future::BoxFuture;
    use futures::{FutureExt as _, TryStreamExt as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream, sink};
    use tokio::join;
    use tokio::task::yield_now;
    use tokio_util::codec::FramedRead;

    use super::*;
    use crate::frame::{Encoder, FrameRef};

    fn frame(path: &[usize], data: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::default();
        Encoder.encode(FrameRef { path, data }, &mut buf).unwrap();
        buf.to_vec()
    }

    fn incoming_paths<P: AsRef<[Option<usize>]>>(
        paths: impl IntoIterator<Item = P>,
    ) -> (
        DuplexStream,
        Incoming<BufReader<DuplexStream>>,
        BoxFuture<'static, std::io::Result<()>>,
    ) {
        let (tx, rx) = tokio::io::duplex(1024);
        let (.., rx) = mux(sink(), BufReader::new(rx), paths);
        let closed = rx.closed().boxed();
        (tx, rx, closed)
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
    async fn conn() {
        let (mut tx, mut rx, done_rx) = incoming_paths([&[Some(0)][..], &[Some(1), None, Some(0)]]);
        let Err(err) = rx.index(&[2]) else {
            panic!("indexing an undeclared path should fail");
        };
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let mut sub = rx.index(&[0]).unwrap();
        let Err(err) = rx.index(&[0]) else {
            panic!("indexing a path twice should fail");
        };
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        let mut prefix = rx.index(&[1, 7]).unwrap();
        let err = prefix.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let mut nested = prefix.index(&[0]).unwrap();
        join!(
            async {
                tx.write_all(
                    &[
                        frame(&[], b"\x2a"),
                        frame(&[0], b"\x2b"),
                        frame(&[1, 7, 0], b"\x2c"),
                        frame(&[1, 7, 0], b""),
                        frame(&[0], b""),
                        frame(&[], b""),
                    ]
                    .concat(),
                )
                .await
                .unwrap();
                drop(tx);
            },
            async {
                let mut buf = Vec::default();
                sub.read_to_end(&mut buf).await.unwrap();
                assert_eq!(buf, b"\x2b");
            },
            async {
                let mut buf = Vec::default();
                nested.read_to_end(&mut buf).await.unwrap();
                assert_eq!(buf, b"\x2c");
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                assert_eq!(buf, b"\x2a");
            },
        );
        done(done_rx).await.unwrap();

        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)]]);
        tx.write_all(&frame(&[], b"")).await.unwrap();
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        drop(rx);
        tx.write_all(&frame(&[0], b"")).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, mut rx, mut done_rx) = incoming_paths([[Some(0)]]);
        tx.write_all(&frame(&[0], b"")).await.unwrap();
        tx.write_all(&frame(&[], b"")).await.unwrap();
        drop(tx);
        assert!((&mut done_rx).now_or_never().is_none());
        let mut sub = rx.index(&[0]).unwrap();
        sub.read_to_end(&mut Vec::default()).await.unwrap();
        drop(sub);
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        drop(rx);
        done(done_rx).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_closes_path_before_nested() {
        let (mut tx, rx, ..) = incoming_paths([[Some(0)].as_slice(), &[Some(0), Some(1)]]);
        let mut parent = rx.index(&[0]).unwrap();
        let mut nested = rx.index(&[0, 1]).unwrap();
        tx.write_all(&frame(&[0], b"")).await.unwrap();
        tx.write_all(&frame(&[0, 1], b"\x2a")).await.unwrap();
        tx.write_all(&frame(&[0, 1], b"")).await.unwrap();
        let mut buf = Vec::default();
        parent.read_to_end(&mut buf).await.unwrap();
        assert!(buf.is_empty());
        nested.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"\x2a");
    }

    #[test_log::test(tokio::test)]
    async fn incoming_closes_root() {
        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)]]);
        let frames = [frame(&[], b"\x2a"), frame(&[], b"")].concat();
        let mut buf = Vec::default();
        let (res, ()) = join!(rx.read_to_end(&mut buf), async {
            tx.write_all(&frames).await.unwrap();
            drop(tx);
        });
        res.unwrap();
        assert_eq!(buf, b"\x2a");
        drop(rx);
        done(done_rx).await.unwrap();

        let (mut tx, mut rx, mut done_rx) = incoming_paths([[Some(0)]]);
        let frames = [frame(&[], b""), frame(&[], b"\x2a")].concat();
        tx.write_all(&frames).await.unwrap();
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        assert!((&mut done_rx).now_or_never().is_none());
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_invalid_frames() {
        for (frames, kind) in [
            (frame(&[0, 0], b"")[..2].to_vec(), ErrorKind::UnexpectedEof),
            (
                [&frame(&[], b"\x2a")[..], &frame(&[0], b"")[..1]].concat(),
                ErrorKind::UnexpectedEof,
            ),
            (
                b"\0\xff\xff\xff\xff\x0f\x42".to_vec(),
                ErrorKind::UnexpectedEof,
            ),
            (b"\xff\xff\xff\xff\x0f\0".to_vec(), ErrorKind::InvalidData),
            (b"\xff\xff\xff\xff\x7f".to_vec(), ErrorKind::InvalidData),
            (frame(&[1], b""), ErrorKind::InvalidData),
            (frame(&[1], b"\x2a"), ErrorKind::InvalidData),
            (frame(&[2, 3, 0], b""), ErrorKind::InvalidData),
        ] {
            let (mut tx, mut rx, done_rx) =
                incoming_paths([[Some(0)].as_slice(), &[Some(1), Some(0)], &[Some(2), None]]);
            let mut buf = Vec::default();
            let ((), res) = join!(
                async {
                    tx.write_all(&frames).await.unwrap();
                    drop(tx);
                },
                rx.read_to_end(&mut buf),
            );
            let err = res.unwrap_err();
            assert_eq!(err.kind(), kind, "{frames:02x?}");
            assert_eq!(failed(done_rx).await, kind, "{frames:02x?}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_unprocessed_data() {
        fn incoming() -> (
            DuplexStream,
            Incoming<BufReader<DuplexStream>>,
            BoxFuture<'static, std::io::Result<()>>,
        ) {
            incoming_paths([[Some(0)]])
        }

        let (.., rx, done_rx) = incoming();
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, mut rx, mut done_rx) = incoming();
        let mut sub = rx.index(&[0]).unwrap();
        let frames = [frame(&[0], b""), frame(&[], b""), frame(&[0], b"")].concat();
        tx.write_all(&frames).await.unwrap();
        join!(
            async { sub.read_to_end(&mut Vec::default()).await.unwrap() },
            async { rx.read_to_end(&mut Vec::default()).await.unwrap() },
        );
        assert!((&mut done_rx).now_or_never().is_none());
        drop(sub);
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (mut tx, rx, done_rx) = incoming();
        let sub = rx.index(&[0]).unwrap();
        tx.write_all(&frame(&[0], b"\x2a")).await.unwrap();
        drop(sub);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
        drop(rx);

        let (mut tx, rx, done_rx) = incoming();
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&frame(&[0], b"\x2a\x2b")).await.unwrap();
        sub.read_exact(&mut [0; 1]).await.unwrap();
        drop(sub);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
        drop(rx);

        let (mut tx, rx, done_rx) = incoming();
        tx.write_all(&frame(&[0], b"\x2a")).await.unwrap();
        tx.write_all(&frame(&[], b"")).await.unwrap();
        drop(tx);
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        let (_tx, mut rx, done_rx) = incoming();
        rx.unread(BytesMut::from(&frame(&[0], b"\x2a")[..]));
        drop(rx);
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);

        for closed_first in [true, false] {
            let (mut tx, rx, done_rx) = incoming();
            if closed_first {
                drop(done_rx);
                drop(rx);
            } else {
                drop(rx);
                yield_now().await;
                drop(done_rx);
            }
            yield_now().await;
            let err = tx.write_all(&frame(&[0], b"\x2a")).await.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::BrokenPipe);
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_unclosed_path_at_end() {
        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)]]);
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&frame(&[0], b"\x2a")).await.unwrap();
        tx.write_all(&frame(&[], b"")).await.unwrap();
        drop(tx);
        let (res, ..) = join!(
            async {
                let mut buf = [0; 1];
                sub.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"\x2a");
                sub.read_to_end(&mut Vec::default()).await
            },
            async { rx.read_to_end(&mut Vec::default()).await.unwrap() },
        );
        assert_eq!(res.unwrap_err().kind(), ErrorKind::UnexpectedEof);
        assert_eq!(failed(done_rx).await, ErrorKind::UnexpectedEof);

        let (mut tx, mut rx, done_rx) = incoming_paths([[Some(0)].as_slice(), &[Some(1)]]);
        let mut open = rx.index(&[0]).unwrap();
        let mut closed = rx.index(&[1]).unwrap();
        tx.write_all(&[frame(&[1], b"\x2a"), frame(&[1], b""), frame(&[], b"")].concat())
            .await
            .unwrap();
        drop(tx);
        let (res, buf, ..) = join!(
            async { open.read_to_end(&mut Vec::default()).await },
            async {
                let mut buf = Vec::default();
                closed.read_to_end(&mut buf).await.unwrap();
                buf
            },
            async { rx.read_to_end(&mut Vec::default()).await.unwrap() },
        );
        assert_eq!(res.unwrap_err().kind(), ErrorKind::UnexpectedEof);
        assert_eq!(buf, b"\x2a");
        assert_eq!(failed(done_rx).await, ErrorKind::UnexpectedEof);
    }

    #[test_log::test(tokio::test)]
    async fn incoming_wildcard_elements() {
        let (mut tx, mut rx, done_rx) = incoming_paths([[None]]);
        let mut first = rx.index(&[1]).unwrap();
        let mut third = rx.index(&[3]).unwrap();
        tx.write_all(&frame(&[3], b"\x2b")).await.unwrap();
        tx.write_all(&frame(&[1], b"\x2a")).await.unwrap();
        let (a, b) = join!(
            async {
                let mut buf = [0; 1];
                first.read_exact(&mut buf).await.unwrap();
                buf
            },
            async {
                let mut buf = [0; 1];
                third.read_exact(&mut buf).await.unwrap();
                buf
            },
        );
        assert_eq!(&a, b"\x2a");
        assert_eq!(&b, b"\x2b");
        tx.write_all(&[frame(&[3], b""), frame(&[1], b""), frame(&[], b"")].concat())
            .await
            .unwrap();
        join!(
            async { third.read_to_end(&mut Vec::default()).await.unwrap() },
            async { first.read_to_end(&mut Vec::default()).await.unwrap() },
            async { rx.read_to_end(&mut Vec::default()).await.unwrap() },
        );
        drop(third);
        drop(first);
        drop(rx);
        tx.write_all(&frame(&[3], b"")).await.unwrap();
        assert_eq!(failed(done_rx).await, ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_writes_frames() {
        let (tx, mut rx) = tokio::io::duplex(4);
        let (mut outgoing, _incoming) = mux(tx, b"".as_slice(), [[None; 0]; 0]);
        let mut sub = outgoing.index(&[1]).unwrap();
        let data = b"\x2a".repeat(32);
        let ((), buf) = join!(
            async {
                sub.write_all(&data).await.unwrap();
                sub.shutdown().await.unwrap();
                outgoing.write_all(b"\x2b").await.unwrap();
                outgoing.flush().await.unwrap();
                drop(sub);
                drop(outgoing);
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                buf
            },
        );
        assert_eq!(
            buf,
            [frame(&[1], &data), frame(&[1], b""), frame(&[], b"\x2b")].concat()
        );
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_writes_frames_unvectored() {
        struct Unvectored(DuplexStream);

        impl AsyncWrite for Unvectored {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.get_mut().0).poll_flush(cx)
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
            }
        }

        let (tx, mut rx) = tokio::io::duplex(64);
        let (outgoing, _incoming) = mux(Unvectored(tx), b"".as_slice(), [[None; 0]; 0]);
        let mut sub = outgoing.index(&[1]).unwrap();
        sub.write_all(b"\x2a").await.unwrap();
        let mut buf = [0; 4];
        tokio::time::timeout(Duration::from_secs(5), rx.read_exact(&mut buf))
            .await
            .expect("frame was not written")
            .unwrap();
        assert_eq!(buf, *frame(&[1], b"\x2a"));
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_continues_partial_frames() {
        let (tx, mut rx) = tokio::io::duplex(4);
        let (mut outgoing, _incoming) = mux(tx, b"".as_slice(), [[None; 0]; 0]);
        let mut sub = outgoing.index(&[1]).unwrap();
        let data = b"\x2a".repeat(32);
        let ((), buf) = join!(
            async {
                join!(
                    async {
                        sub.write_all(&data).await.unwrap();
                        sub.shutdown().await.unwrap();
                        drop(sub);
                    },
                    async {
                        outgoing.write_all(b"\x2b").await.unwrap();
                        outgoing.shutdown().await.unwrap();
                        drop(outgoing);
                    },
                );
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                buf
            },
        );
        let mut rx = FramedRead::new(buf.as_slice(), crate::frame::Decoder::default());
        let mut sub_data = Vec::default();
        let mut root_data = Vec::default();
        let mut closes = 0;
        while let Some(crate::frame::Frame { path, data }) = rx.try_next().await.unwrap() {
            if data.is_empty() {
                closes += 1;
            }
            match &*path {
                [1] => sub_data.extend_from_slice(&data),
                [] => root_data.extend_from_slice(&data),
                path => panic!("unexpected path {path:?}"),
            }
        }
        assert_eq!(sub_data, data);
        assert_eq!(root_data, b"\x2b");
        assert_eq!(closes, 2);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_reports_egress_failure() {
        struct FailingWriter;

        impl AsyncWrite for FailingWriter {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Poll::Ready(Err(std::io::Error::other("test")))
            }

            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let (mut tx, mut rx) = mux(FailingWriter, b"\0\0".as_slice(), [[None; 0]; 0]);
        let err = tx.write_all(b"foo").await.unwrap_err();
        assert_eq!(err.to_string(), "test");
        let err = tx.write_all(b"bar").await.unwrap_err();
        assert_eq!(err.to_string(), "test");
        let err = rx.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Other);
    }

    #[test_log::test(tokio::test)]
    async fn ingress_failure_aborts_egress() {
        let (mut tx, rx) = tokio::io::duplex(16);
        let (mut outgoing, incoming) = mux(sink(), BufReader::new(rx), [[None; 0]; 0]);
        tx.write_all(&frame(&[1], b"\x2a")).await.unwrap();
        let err = incoming.closed().await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        let err = outgoing.write_all(b"x").await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        drop(incoming);
        let err = tx.write_all(b"x").await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }
}
