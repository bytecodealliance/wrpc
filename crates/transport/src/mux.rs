use core::future::poll_fn;
use core::iter::zip;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};

use std::collections::BTreeMap;
use std::io::IoSlice;
use std::sync::{Arc, Weak};

use bytes::{Buf, BufMut as _, Bytes, BytesMut};
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::select;
use tokio_util::codec::{Decoder, Encoder as _};
use tokio_util::io::poll_read_buf;
use tracing::{debug, error, instrument, trace};
use wasm_tokio::{Leb128Encoder, put_u32_leb128};

use crate::frame::{FrameHeader, HeaderDecoder};
use crate::{Deferred as _, DeferredWrite, Encode, Index, READ_CHUNK_SIZE};

/// Maximum number of bytes of encoded stream chunks buffered before they are written
const MAX_PENDING_STREAM_BATCH: usize = 1 << 20;

/// State of an invocation shared by its handles
struct IncomingState<T> {
    stream: T,
    /// Declared paths
    paths: Box<[Box<[Option<usize>]>]>,
    /// Decoder of the frame headers
    header: HeaderDecoder,
    /// Bytes received, which were not processed yet: at most one read of the stream and a part
    /// of a frame header
    rx_buf: BytesMut,
    /// Path of the frame being received, once its header is complete
    frame: Option<Arc<[usize]>>,
    /// Length of the data of the frame being received left to receive
    remaining: usize,
    /// Whether ingress ended
    ended: bool,
    /// Waker of the root [Incoming] handle and of each one, which read from its path, but not
    /// its end, keyed by the path, set while the handle waits for ingress
    readers: BTreeMap<Box<[usize]>, Option<Waker>>,
    /// Wakers of the [Closed] futures
    closed: Vec<Waker>,
    /// Number of [Incoming] handles
    incoming: usize,
    failed: bool,
}

impl<T: AsyncRead + Unpin> IncomingState<T> {
    fn fail(&mut self) {
        self.failed = true;
        self.wake();
    }

    /// Fails the invocation with `err`, unless it failed already, waking all handles
    fn fail_err(&mut self, err: std::io::Error) -> std::io::Error {
        if !self.failed {
            error!(?err, "invocation failed");
            self.failed = true;
            self.wake();
        }
        err
    }

    /// Wakes all handles and [Closed] futures
    fn wake(&mut self) {
        self.readers
            .values_mut()
            .filter_map(Option::take)
            .for_each(Waker::wake);
        self.closed.drain(..).for_each(Waker::wake);
    }

    /// Receives the header of the next frame with `cx` and wakes the reader of its path, or ends
    /// ingress at the end of the stream
    fn poll_frame(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let header = loop {
            let header = self.header.decode(&mut self.rx_buf)?;
            if header.is_some() {
                break Ok(header);
            }
            self.rx_buf.reserve(READ_CHUNK_SIZE);
            let n = ready!(poll_read_buf(
                Pin::new(&mut self.stream),
                cx,
                &mut self.rx_buf
            ))?;
            if n == 0 {
                break self.header.decode_eof(&mut self.rx_buf);
            }
        };
        let Some(FrameHeader { path, size: n }) = header? else {
            if !self.readers.is_empty() {
                return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
            }
            debug!("ingress complete");
            self.ended = true;
            self.closed.drain(..).for_each(Waker::wake);
            return Poll::Ready(Ok(()));
        };
        trace!(?path, n, "received frame header");
        match self.readers.get_mut(&*path) {
            Some(waker) => waker.take().into_iter().for_each(Waker::wake),
            None if resolve(&self.paths, &path) != Some(true) => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("received frame on undeclared path `{path:?}`"),
                )));
            }
            None if self.incoming == 0 => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "received data was not processed",
                )));
            }
            None => trace!("holding frame until the path is read"),
        }
        self.frame = Some(path);
        self.remaining = n;
        Poll::Ready(Ok(()))
    }

    /// Reads the data of the frame on `path` into `buf` on behalf of its [Incoming] handle,
    /// receiving frames with `cx` until one is on `path`, and returns whether the path is closed.
    /// Once the frame is read, receiving the next one is handed off.
    fn poll_read(
        &mut self,
        path: &[usize],
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<bool>> {
        if self.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        match self.readers.get_mut(path) {
            Some(reader) => *reader = None,
            None => {
                self.readers.insert(path.into(), None);
            }
        }
        loop {
            let Some(frame) = &self.frame else {
                if self.ended {
                    return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
                }
                let Poll::Ready(()) = self.poll_frame(cx)? else {
                    break;
                };
                continue;
            };
            if **frame != *path {
                trace!(?frame, "waiting for the frame of another path to be read");
                break;
            }
            let closed = self.remaining == 0;
            if !closed && self.rx_buf.is_empty() {
                self.rx_buf.reserve(READ_CHUNK_SIZE);
                let Poll::Ready(n) =
                    poll_read_buf(Pin::new(&mut self.stream), cx, &mut self.rx_buf)?
                else {
                    break;
                };
                if n == 0 {
                    return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
                }
            }
            let n = buf.remaining().min(self.rx_buf.len()).min(self.remaining);
            buf.put_slice(&self.rx_buf[..n]);
            self.rx_buf.advance(n);
            self.remaining = self.remaining.saturating_sub(n);
            if closed {
                trace!("closing path");
                self.readers.remove(path);
            }
            if self.remaining == 0 {
                self.frame = None;
                self.readers
                    .values_mut()
                    .filter_map(Option::take)
                    .for_each(Waker::wake);
                self.closed.drain(..).for_each(Waker::wake);
            }
            return Poll::Ready(Ok(closed));
        }
        if let Some(reader) = self.readers.get_mut(path) {
            *reader = Some(cx.waker().clone());
        }
        Poll::Pending
    }

    /// Receives frames with `cx` on behalf of a [Closed] future until ingress ends
    fn poll_closed(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        if !self.ended && self.frame.is_none() {
            let Poll::Ready(()) = self.poll_frame(cx)? else {
                return pending(&mut self.closed, cx);
            };
        }
        if !self.ended {
            return pending(&mut self.closed, cx);
        }
        Poll::Ready(Ok(()))
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

/// Returns the error of an operation of a failed invocation
fn error_invocation_failed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "invocation failed")
}

/// Registers the waker of `cx` in `wakers`
fn pending<T>(wakers: &mut Vec<Waker>, cx: &Context<'_>) -> Poll<T> {
    if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
        wakers.push(cx.waker().clone());
    }
    Poll::Pending
}

/// Fails egress of an invocation, which failed
fn fail_outgoing<T>(outgoing: &Weak<std::sync::Mutex<OutgoingState<T>>>) {
    if let Some(state) = outgoing.upgrade() {
        let mut state = lock(&state);
        state.failed = true;
        state.wake();
    }
}

/// Locks `mutex`, recovering it if a thread panicked while holding the lock
fn lock<T: ?Sized>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Multiplexes the [`AsyncWrite`] and [`AsyncRead`] of an invocation declaring the async
/// `paths`, which sends `tx_buf` before the first frame and receives `rx_buf` before the bytes
/// read from `rx`, returning the [Outgoing] and [Incoming] handles of the root path.
///
/// The streams are polled by the handles, nothing is spawned: data is received only while a
/// handle reads or the future returned by [`Incoming::closed`] is polled, and sent only while a
/// handle writes, flushes, shuts down or closes. A frame is received as a whole before the next one and
/// held until the handle of its path reads it, so all handles of an invocation must be read
/// concurrently.
/// The root [Incoming] handle and a handle, which read data of its path, must read the end of
/// it before it is dropped. Other handles of paths, which carry no value, such as the elements
/// of a list of futures, need not be read. Every [Outgoing] handle must be closed with
/// [`Outgoing::close`], the last one shuts the write half of the stream down, which is otherwise
/// dropped with the last handle without being shut down. The read half is dropped with the last
/// [Incoming] handle and [Closed] future.
/// A handle of a declared path, which did not read data, is not known to expect any, since
/// merged variant paths may carry none, so a frame received on its path after it was dropped,
/// or on a closed path, is held like one, whose handle was not indexed yet, stalling ingress
/// until all handles are dropped.
/// The invocation fails on a frame received on a path, which is not declared, on a frame
/// received on a path no handle reads once all handles are dropped, on the stream ending before
/// a handle read the end of its path, on a handle dropped before it read the end of its path
/// and on an error of the stream. Every handle of a failed invocation fails with
/// [`std::io::ErrorKind::BrokenPipe`].
///
/// `paths` must not contain both a wildcard and an index at the same position, so paths of
/// different variant cases, which are not indexed, must be merged by replacing such indices
/// by wildcards.
pub fn mux(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    tx_buf: BytesMut,
    rx_buf: BytesMut,
    paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
) -> (Outgoing, Incoming) {
    let paths: Box<[Box<[Option<usize>]>]> = paths
        .into_iter()
        .map(|path| Box::<[Option<usize>]>::from(path.as_ref()))
        .filter(|path| !path.is_empty())
        .collect();
    let depth = paths.iter().map(|path| path.len()).max().unwrap_or(0);
    let depth = u32::try_from(depth).unwrap_or(u32::MAX);
    let header = HeaderDecoder::new(depth, u32::MAX);
    let outgoing_state = Arc::new(std::sync::Mutex::new(OutgoingState {
        buffer: tx_buf,
        stream: Box::pin(tx) as _,
        pending: Vec::default(),
        failed: false,
        written: 0,
    }));
    let incoming_state = Arc::new(std::sync::Mutex::new(IncomingState {
        stream: Box::pin(rx) as _,
        paths,
        header,
        rx_buf,
        frame: None,
        remaining: 0,
        ended: false,
        readers: BTreeMap::from([(Box::default(), None)]),
        closed: Vec::default(),
        incoming: 1,
        failed: false,
    }));
    let incoming = Arc::downgrade(&incoming_state);
    let outgoing = Arc::downgrade(&outgoing_state);
    (
        Outgoing {
            path: Box::default(),
            path_buf: Bytes::from_static(&[0]),
            closed: 0,
            state: outgoing_state,
            incoming,
        },
        Incoming {
            path: Box::default(),
            closed: false,
            buffer: BytesMut::default(),
            state: incoming_state,
            outgoing,
        },
    )
}

/// Incoming framed stream
pub struct Incoming {
    path: Box<[usize]>,
    /// Whether this handle read the end of the data of its path, which the handle of a path,
    /// which is not declared, carries none of
    closed: bool,
    /// Bytes read past the item decoded by [`Self::decode`], which are read before the rest of
    /// the data of the path
    buffer: BytesMut,
    state: Arc<std::sync::Mutex<IncomingState<Pin<Box<dyn AsyncRead + Send + Unpin>>>>>,
    outgoing: Weak<std::sync::Mutex<OutgoingState<Pin<Box<dyn AsyncWrite + Send + Unpin>>>>>,
}

impl Drop for Incoming {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        state.incoming = state.incoming.saturating_sub(1);
        if state.readers.remove(&self.path).is_some() {
            trace!(path = ?self.path, "dropping handle of an open path");
            state.fail_err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("`{:?}` dropped before it was closed", self.path),
            ));
        }
        let unread = state
            .frame
            .as_ref()
            .is_some_and(|frame| state.incoming == 0 || **frame == *self.path);
        if !self.buffer.is_empty() || unread {
            trace!(path = ?self.path, "dropping handle with unprocessed data");
            state.fail_err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "received data was not processed",
            ));
        }
        let failed = state.failed;
        drop(state);
        if failed {
            fail_outgoing(&self.outgoing);
        }
    }
}

impl Incoming {
    /// Index the incoming stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it. A declared path must be indexed at most once, the
    /// handle of a path, which is not declared, carries no data and only serves to index the
    /// paths nested in it.
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        let path: Box<[usize]> = [&self.path, path].concat().into();
        let mut state = lock(&self.state);
        let Some(declared) = resolve(&state.paths, &path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("`{path:?}` is not declared"),
            ));
        };
        state.incoming = state.incoming.saturating_add(1);
        Ok(Self {
            path,
            closed: !declared,
            buffer: BytesMut::default(),
            state: Arc::clone(&self.state),
            outgoing: Weak::clone(&self.outgoing),
        })
    }

    /// Returns a future, which receives ingress until the peer ends transmit and resolves to the
    /// result of the invocation once ingress ends
    #[must_use]
    pub fn closed(&self) -> Closed {
        Closed {
            state: Arc::clone(&self.state),
            outgoing: Weak::clone(&self.outgoing),
        }
    }

    /// Decodes the next item of the path using `dec`, returning [None] if the path ended
    /// without one
    pub async fn decode<D: Decoder>(&mut self, dec: &mut D) -> Result<Option<D::Item>, D::Error> {
        loop {
            if !self.buffer.is_empty()
                && let Some(item) = dec.decode(&mut self.buffer)?
            {
                return Ok(Some(item));
            }
            let n = poll_fn(|cx| {
                let mut buf = mem::take(&mut self.buffer);
                buf.reserve(READ_CHUNK_SIZE);
                let res = poll_read_buf(Pin::new(&mut *self), cx, &mut buf);
                self.buffer = buf;
                res
            })
            .await?;
            if n == 0 {
                return dec.decode_eof(&mut self.buffer);
            }
        }
    }

    /// Reads the end of the data of the path, if this handle read any, failing on data left
    pub async fn read_end(&mut self) -> std::io::Result<()> {
        let open = lock(&self.state).readers.contains_key(&self.path);
        if !self.closed && self.buffer.is_empty() && !open {
            return Ok(());
        }
        let n = self.read(&mut [0]).await?;
        if n > 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "received data after value end",
            ));
        }
        Ok(())
    }
}

impl AsyncRead for Incoming {
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
        if !this.buffer.is_empty() {
            let n = buf.remaining().min(this.buffer.len());
            buf.put_slice(&this.buffer.split_to(n));
            return Poll::Ready(Ok(()));
        }
        if this.closed {
            return Poll::Ready(Ok(()));
        }
        let res = {
            let mut state = lock(&this.state);
            let res = ready!(state.poll_read(&this.path, cx, buf));
            res.map_err(|err| state.fail_err(err))
        };
        if res.is_err() {
            fail_outgoing(&this.outgoing);
        }
        this.closed = res?;
        Poll::Ready(Ok(()))
    }
}

/// Future returned by [`Incoming::closed`]
pub struct Closed {
    state: Arc<std::sync::Mutex<IncomingState<Pin<Box<dyn AsyncRead + Send + Unpin>>>>>,
    outgoing: Weak<std::sync::Mutex<OutgoingState<Pin<Box<dyn AsyncWrite + Send + Unpin>>>>>,
}

impl Drop for Closed {
    fn drop(&mut self) {
        lock(&self.state).wake();
    }
}

impl Future for Closed {
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = {
            let mut state = lock(&self.state);
            let res = ready!(state.poll_closed(cx));
            res.map_err(|err| state.fail_err(err))
        };
        if res.is_err() {
            fail_outgoing(&self.outgoing);
        }
        Poll::Ready(res)
    }
}

struct OutgoingState<T> {
    buffer: BytesMut,
    stream: T,
    pending: Vec<Waker>,
    failed: bool,
    written: usize,
}

impl<T> OutgoingState<T> {
    fn wake(&mut self) {
        for w in mem::take(&mut self.pending) {
            w.wake();
        }
    }
}

/// Outgoing framed stream
pub struct Outgoing {
    path: Box<[usize]>,
    path_buf: Bytes,
    closed: usize,
    state: Arc<std::sync::Mutex<OutgoingState<Pin<Box<dyn AsyncWrite + Send + Unpin>>>>>,
    incoming: Weak<std::sync::Mutex<IncomingState<Pin<Box<dyn AsyncRead + Send + Unpin>>>>>,
}

impl Outgoing {
    /// Returns the number of bytes buffered, which have not been flushed yet.
    pub fn buffered(&self) -> usize {
        lock(&self.state).buffer.len()
    }

    fn fail<T>(&self, state: &mut OutgoingState<T>) {
        state.failed = true;
        if let Some(state) = self.incoming.upgrade() {
            lock(&state).fail();
        }
    }

    /// Shuts the handle down and drops it, shutting the stream down if it is the last handle
    pub async fn close(mut self) -> std::io::Result<()> {
        self.shutdown().await?;
        let Some(state) = Arc::into_inner(self.state) else {
            return Ok(());
        };
        let OutgoingState { mut stream, .. } = state
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = stream.shutdown().await {
            if let Some(state) = self.incoming.upgrade() {
                lock(&state).fail();
            }
            return Err(err);
        }
        Ok(())
    }

    fn poll_write(
        &self,
        state: &mut OutgoingState<Pin<Box<dyn AsyncWrite + Send + Unpin>>>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let n = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let mut n_buf = [0; 5];
        let n_buf = put_u32_leb128(&mut n_buf, n);
        let buf = &buf[..n as _];
        let res = if state.stream.is_write_vectored() {
            loop {
                let buffered = state.buffer.len();
                let res = if buffered == 0 {
                    state.stream.as_mut().poll_write_vectored(
                        cx,
                        &[
                            IoSlice::new(&self.path_buf),
                            IoSlice::new(n_buf),
                            IoSlice::new(buf),
                        ],
                    )
                } else {
                    state.stream.as_mut().poll_write_vectored(
                        cx,
                        &[
                            IoSlice::new(&state.buffer),
                            IoSlice::new(&self.path_buf),
                            IoSlice::new(n_buf),
                            IoSlice::new(buf),
                        ],
                    )
                };
                let Poll::Ready(res) = res else {
                    return pending(&mut state.pending, cx);
                };
                state.wake();
                match res {
                    Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
                    Ok(w) => {
                        state.written = state.written.saturating_add(w);
                        let Some(w) = w.checked_sub(buffered) else {
                            state.buffer.advance(w);
                            continue;
                        };
                        state.buffer.clear();
                        let Some(w) = w.checked_sub(self.path_buf.len()) else {
                            let path_buf = &self.path_buf[w..];
                            state.buffer.reserve(
                                path_buf
                                    .len()
                                    .saturating_add(n_buf.len())
                                    .saturating_add(buf.len()),
                            );
                            state.buffer.extend_from_slice(path_buf);
                            state.buffer.extend_from_slice(n_buf);
                            state.buffer.extend_from_slice(buf);
                            break Ok(buf.len());
                        };
                        let Some(w) = w.checked_sub(n_buf.len()) else {
                            let n_buf = &n_buf[w..];
                            state.buffer.reserve(n_buf.len().saturating_add(buf.len()));
                            state.buffer.extend_from_slice(n_buf);
                            state.buffer.extend_from_slice(buf);
                            break Ok(buf.len());
                        };
                        if w < buf.len() {
                            let buf = &buf[w..];
                            state.buffer.reserve(buf.len());
                            state.buffer.extend_from_slice(buf);
                        };
                        break Ok(buf.len());
                    }
                    Err(err) => break Err(err),
                }
            }
        } else {
            let mut buffered = state.buffer.len();
            state.buffer.extend_from_slice(&self.path_buf);
            state.buffer.extend_from_slice(n_buf);
            state.buffer.extend_from_slice(buf);
            loop {
                let res = state.stream.as_mut().poll_write(cx, &state.buffer);
                let Poll::Ready(res) = res else {
                    state.buffer.truncate(buffered);
                    return pending(&mut state.pending, cx);
                };
                state.wake();
                match res {
                    Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
                    Ok(w) => {
                        state.written = state.written.saturating_add(w);
                        state.buffer.advance(w);
                        if w < buffered {
                            buffered = buffered.saturating_sub(w);
                            continue;
                        }
                        break Ok(buf.len());
                    }
                    Err(err) => break Err(err),
                }
            }
        };
        if res.is_err() {
            self.fail(state);
        }
        Poll::Ready(res)
    }

    fn poll_flush(
        &self,
        state: &mut OutgoingState<Pin<Box<dyn AsyncWrite + Send + Unpin>>>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let res = loop {
            if state.buffer.is_empty() {
                let Poll::Ready(res) = state.stream.as_mut().poll_flush(cx) else {
                    return pending(&mut state.pending, cx);
                };
                state.wake();
                break res;
            }
            let Poll::Ready(res) = state.stream.as_mut().poll_write(cx, &state.buffer) else {
                return pending(&mut state.pending, cx);
            };
            state.wake();
            match res {
                Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    state.buffer.advance(n);
                    state.written = state.written.saturating_add(n);
                }
                Err(err) => break Err(err),
            }
        };
        if res.is_err() {
            self.fail(state);
        }
        Poll::Ready(res)
    }
}

impl Index for Outgoing {
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        let path: Box<[usize]> = [&self.path, path].concat().into();
        let mut path_buf = BytesMut::default();
        for n in [path.len()].iter().chain(&path) {
            let n = u32::try_from(*n)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            path_buf.put_slice(put_u32_leb128(&mut [0; 5], n));
        }
        Ok(Self {
            path,
            path_buf: path_buf.freeze(),
            closed: 0,
            state: Arc::clone(&self.state),
            incoming: Weak::clone(&self.incoming),
        })
    }
}

/// Writes to the stream as frames, buffering the bytes the stream did not accept, which are
/// written before the next frame of any path. Shutting the handle down closes the path by
/// writing an empty frame and resolves once it is written, the stream is shut down by
/// [`Outgoing::close`]. Writing to a handle after it was shut down is not supported.
impl AsyncWrite for Outgoing {
    #[instrument(
        level = "trace",
        skip_all,
        fields(path = ?self.path, buf = format!("{buf:02x?}")),
        ret(level = "trace")
    )]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut state = lock(&self.state);
        if state.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        self.as_ref().poll_write(&mut state, cx, buf)
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut state = lock(&self.state);
        if state.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        self.as_ref().poll_flush(&mut state, cx)
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut state = lock(&this.state);
        if state.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        if this.closed == 0 {
            ready!(this.poll_write(&mut state, cx, &[]))?;
            this.closed = state.written.saturating_add(state.buffer.len());
        }
        if state.written < this.closed {
            ready!(this.poll_flush(&mut state, cx))?;
        }
        Poll::Ready(Ok(()))
    }
}

impl DeferredWrite for Outgoing {
    async fn write_future<T>(
        mut self,
        _offset: usize,
        item: impl Future<Output = T> + Send + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self>,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let item = item.await;
        let mut enc = T::Encoder::default();
        let mut buf = BytesMut::default();
        enc.encode(item, &mut buf)?;
        self.write_all(&buf).await?;
        let nested = enc.take_deferred().map(|f| f(&self, &[0]));
        let nested = nested.transpose()?;
        self.close().await?;
        let Some(nested) = nested else {
            return Ok(());
        };
        nested.await
    }

    async fn write_stream<T>(
        mut self,
        _offset: usize,
        mut items: impl Stream<Item = Vec<T>> + Send + Unpin + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self> + Send + 'static,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let mut enc = T::Encoder::default();
        let mut buf = BytesMut::default();
        let mut deferred = Vec::default();
        let mut tasks = FuturesUnordered::default();
        let mut pending = false;
        let mut ended = false;
        let mut closed = false;
        let mut i = 0_u64;
        while !closed {
            select! {
                chunk = items.next(), if !ended && buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let Some(chunk) = chunk else {
                        trace!("writing stream end");
                        ended = true;
                        continue;
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    let n = u32::try_from(chunk.len()).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    let end = i.checked_add(n.into()).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "stream element index would overflow u64",
                        )
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    trace!(i, buf = format!("{buf:02x?}"), "writing stream chunk items");

                    buf.reserve(chunk.len());
                    for (i, item) in zip(i.., chunk) {
                        enc.encode(item, &mut buf)?;
                        if let Some(f) = enc.take_deferred() {
                            let i = usize::try_from(i).map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                            })?;
                            let fut = f(&self, &[i])?;
                            deferred.push(fut);
                        }
                    }
                    i = end;
                }
                Some(res) = tasks.next() => {
                    trace!(?res, "receiver task finished");
                    res?;
                }
                res = async {
                    if buf.is_empty() {
                        self.flush().await.map(|()| 0)
                    } else {
                        trace!(?buf, "writing bytes from buffer");
                        self.write(&buf).await
                    }
                }, if pending || !buf.is_empty() || (ended && !closed) => {
                    let n = res?;
                    if buf.is_empty() {
                        pending = false;
                        closed = ended;
                        tasks.extend(deferred.drain(..));
                        continue;
                    }
                    buf.advance(n);
                    if buf.is_empty() {
                        pending = true;
                        tasks.extend(deferred.drain(..));
                    }
                }
            }
        }
        self.close().await?;
        while let Some(res) = tasks.next().await {
            res?;
        }
        Ok(())
    }

    async fn write_stream_bytes(
        mut self,
        _offset: usize,
        mut items: impl Stream<Item = Bytes> + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        let mut buf = BytesMut::default();
        let mut pending = false;
        loop {
            select! {
                chunk = items.next(), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let Some(chunk) = chunk else {
                        trace!("writing stream end");
                        self.write_all(&buf).await?;
                        return self.close().await
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    let n = u32::try_from(chunk.len()).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    buf.extend_from_slice(&chunk);
                }
                res = self.flush(), if pending || !buf.is_empty() => {
                    res?;
                    pending = false;
                    if !buf.is_empty() {
                        self.write_all(&buf).await?;
                        buf.clear();
                        pending = true;
                    }
                }
            }
        }
    }

    async fn write_stream_read(
        mut self,
        _offset: usize,
        mut buf: BytesMut,
        mut items: impl AsyncRead + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        if !buf.is_empty() {
            let n = u32::try_from(buf.len())
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(n, "writing ready chunk length");
            let mut header = BytesMut::default();
            Leb128Encoder.encode(n, &mut header)?;
            self.write_all(&header).await?;
        }
        let mut chunk = BytesMut::with_capacity(READ_CHUNK_SIZE);
        let mut pending = false;
        loop {
            select! {
                res = items.read_buf(&mut chunk), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let n = res?;
                    if n == 0 {
                        trace!("writing stream end");
                        self.write_all(&buf).await?;
                        return self.close().await
                    }
                    let n = u32::try_from(n).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    buf.extend_from_slice(&chunk);
                    chunk.clear();
                }
                res = self.flush(), if pending || !buf.is_empty() => {
                    res?;
                    pending = false;
                    if !buf.is_empty() {
                        self.write_all(&buf).await?;
                        buf.clear();
                        pending = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use futures::TryStreamExt as _;
    use tokio::io::{DuplexStream, duplex, sink};
    use tokio::join;
    use tokio::task::yield_now;
    use tokio_util::codec::FramedRead;

    use super::*;
    use crate::frame::{Decoder, Encoder, Frame, FrameRef};

    fn frame(path: &[usize], data: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::default();
        Encoder.encode(FrameRef { path, data }, &mut buf).unwrap();
        buf.to_vec()
    }

    async fn incoming(
        paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
    ) -> (DuplexStream, Incoming, Closed) {
        let (tx, rx) = duplex(1024);
        let (mut root, rx) = mux(sink(), rx, BytesMut::default(), BytesMut::default(), paths);
        root.shutdown().await.unwrap();
        let closed = rx.closed();
        (tx, rx, closed)
    }

    async fn wait(closed: Closed) -> std::io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("invocation did not end")
    }

    #[test_log::test(tokio::test)]
    async fn incoming_frames() {
        let root = frame(&[], b"\x2a");
        let (mut tx, rx) = duplex(1);
        let (mut outgoing, mut rx) = mux(
            sink(),
            rx,
            BytesMut::default(),
            BytesMut::from(&root[..1]),
            [&[Some(0)][..], &[Some(0), None, Some(0)]],
        );
        outgoing.shutdown().await.unwrap();
        let closed = rx.closed();
        let Err(err) = rx.index(&[1]) else {
            panic!("indexing an undeclared path should fail");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let mut sub = rx.index(&[0]).unwrap();
        let mut nested = rx.index(&[0, 7, 0]).unwrap();
        let frames = [
            &root[1..],
            &frame(&[0], b"\x2b"),
            &frame(&[0], b""),
            &frame(&[0, 7, 0], b"\x2c\x2d"),
            &frame(&[0, 7, 0], b""),
            &frame(&[], b""),
        ]
        .concat();
        let ((), sub_buf, nested_buf, rx_buf) = join!(
            async {
                tx.write_all(&frames).await.unwrap();
                drop(tx);
            },
            async {
                let mut buf = Vec::default();
                sub.read_to_end(&mut buf).await.unwrap();
                buf
            },
            async {
                let mut buf = Vec::default();
                nested.read_to_end(&mut buf).await.unwrap();
                buf
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                buf
            },
        );
        assert_eq!(sub_buf, b"\x2b");
        assert_eq!(nested_buf, b"\x2c\x2d");
        assert_eq!(rx_buf, b"\x2a");
        drop(sub);
        drop(nested);
        drop(rx);
        wait(closed).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_wakes_readers() {
        let (mut tx, mut rx, closed) = incoming([[Some(0)]]).await;
        let mut sub = rx.index(&[0]).unwrap();
        let sub = tokio::spawn(async move {
            let mut buf = Vec::default();
            sub.read_to_end(&mut buf).await.unwrap();
            buf
        });
        yield_now().await;
        let frames = [
            frame(&[0], b"\x2b"),
            frame(&[0], b""),
            frame(&[], b"\x2a"),
            frame(&[], b""),
        ]
        .concat();
        tx.write_all(&frames).await.unwrap();
        let mut buf = Vec::default();
        tokio::time::timeout(Duration::from_secs(5), rx.read_to_end(&mut buf))
            .await
            .expect("frame was not received")
            .unwrap();
        assert_eq!(buf, b"\x2a");
        let buf = tokio::time::timeout(Duration::from_secs(5), sub)
            .await
            .expect("frame was not received")
            .unwrap();
        assert_eq!(buf, b"\x2b");
        drop(tx);
        drop(rx);
        wait(closed).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn incoming_rejects_invalid_frames() {
        for (frames, kind) in [
            (
                frame(&[0], b"\x2a")[..2].to_vec(),
                std::io::ErrorKind::UnexpectedEof,
            ),
            (
                b"\0\xff\xff\xff\xff\x0f\x2a".to_vec(),
                std::io::ErrorKind::UnexpectedEof,
            ),
            (
                b"\xff\xff\xff\xff\x0f\0".to_vec(),
                std::io::ErrorKind::InvalidData,
            ),
            (frame(&[1], b""), std::io::ErrorKind::InvalidData),
        ] {
            let (mut tx, mut rx, closed) = incoming([[Some(0)]]).await;
            let mut buf = Vec::default();
            let ((), res) = join!(
                async {
                    tx.write_all(&frames).await.unwrap();
                    drop(tx);
                },
                rx.read_to_end(&mut buf),
            );
            assert_eq!(res.unwrap_err().kind(), kind, "{frames:02x?}");
            let err = wait(closed).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "{frames:02x?}");
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_fails_fast() {
        let (mut tx, rx, closed) = incoming([[Some(0)]]).await;
        tx.write_all(&frame(&[], b"\x2a")).await.unwrap();
        drop(rx);
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);

        let (mut tx, rx, closed) = incoming([[Some(0)]]).await;
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&frame(&[0], b"\x2a\x2b")).await.unwrap();
        sub.read_exact(&mut [0; 1]).await.unwrap();
        drop(sub);
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        drop(rx);

        let (mut tx, mut rx, closed) = incoming([[Some(0)]]).await;
        tx.write_all(&frame(&[], b"")).await.unwrap();
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        drop(rx);
        tx.write_all(&frame(&[0], b"")).await.unwrap();
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_frames() {
        let (tx, mut rx) = duplex(4);
        let (mut outgoing, _incoming) = mux(
            tx,
            b"".as_slice(),
            BytesMut::default(),
            BytesMut::default(),
            [[None; 0]; 0],
        );
        let mut sub = outgoing.index(&[1]).unwrap();
        let mut nested = sub.index(&[2]).unwrap();
        let data = b"\x2a".repeat(32);
        let ((), (), (), buf) = join!(
            async {
                outgoing.write_all(b"\x2b").await.unwrap();
                outgoing.close().await.unwrap();
            },
            async {
                sub.write_all(&data).await.unwrap();
                sub.close().await.unwrap();
            },
            async {
                nested.write_all(b"\x2c").await.unwrap();
                nested.close().await.unwrap();
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                buf
            },
        );
        let frames: Vec<Frame> = FramedRead::new(buf.as_slice(), Decoder::default())
            .try_collect()
            .await
            .unwrap();
        for (path, expected) in [(&[][..], &b"\x2b"[..]), (&[1], &data), (&[1, 2], b"\x2c")] {
            let data: Vec<_> = frames
                .iter()
                .filter(|frame| *frame.path == *path)
                .map(|frame| &frame.data[..])
                .collect();
            let Some((close, data)) = data.split_last() else {
                panic!("no frames on {path:?}");
            };
            assert!(close.is_empty());
            assert_eq!(data.concat(), expected);
        }
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_partial_writes() {
        struct Limited {
            tx: Arc<std::sync::Mutex<Vec<u8>>>,
            ready: bool,
            vectored: bool,
        }

        impl AsyncWrite for Limited {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                self.poll_write_vectored(cx, &[std::io::IoSlice::new(buf)])
            }

            fn poll_write_vectored(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bufs: &[std::io::IoSlice<'_>],
            ) -> Poll<std::io::Result<usize>> {
                let this = self.get_mut();
                this.ready = !this.ready;
                if !this.ready {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let mut tx = this.tx.lock().unwrap();
                let mut n = 0;
                for buf in bufs {
                    let m = buf.len().min(1000 - n);
                    tx.extend_from_slice(&buf[..m]);
                    n += m;
                }
                Poll::Ready(Ok(n))
            }

            fn is_write_vectored(&self) -> bool {
                self.vectored
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

        let data: Vec<u8> = (0..1 << 18).map(|i: usize| i.to_le_bytes()[0]).collect();
        for vectored in [true, false] {
            let tx = Arc::default();
            let (mut outgoing, _incoming) = mux(
                Limited {
                    tx: Arc::clone(&tx),
                    ready: false,
                    vectored,
                },
                b"".as_slice(),
                BytesMut::default(),
                BytesMut::default(),
                [[None; 0]; 0],
            );
            outgoing.write_all(&data).await.unwrap();
            outgoing.shutdown().await.unwrap();
            let tx = tx.lock().unwrap().clone();
            let frames: Vec<Frame> = FramedRead::new(tx.as_slice(), Decoder::default())
                .try_collect()
                .await
                .unwrap();
            let Some((close, frames)) = frames.split_last() else {
                panic!("no frames sent");
            };
            assert!(close.data.is_empty());
            assert!(frames.iter().all(|frame| !frame.data.is_empty()));
            let rx_data: Vec<u8> = frames
                .iter()
                .flat_map(|frame| frame.data.to_vec())
                .collect();
            assert_eq!(rx_data, data);
        }
    }

    #[test_log::test(tokio::test)]
    async fn closing_rules() {
        let (mut tx, mut rx, closed) = incoming([[Some(0)]]).await;
        let mut sub = rx.index(&[0]).unwrap();
        tx.write_all(&frame(&[], b"")).await.unwrap();
        drop(tx);
        rx.read_to_end(&mut Vec::default()).await.unwrap();
        let err = sub.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn final_flush() {
        let (tx, mut rx) = duplex(1);
        let (outgoing, _incoming) = mux(
            tx,
            b"".as_slice(),
            BytesMut::default(),
            BytesMut::default(),
            [[None; 0]; 0],
        );
        let mut buf = Vec::default();
        let ((), n) = join!(
            async {
                outgoing.close().await.unwrap();
            },
            rx.read_to_end(&mut buf),
        );
        n.unwrap();
        assert_eq!(buf, b"\0\0");
    }

    #[test_log::test(tokio::test)]
    async fn failure_aborts_invocation() {
        struct Failing;

        impl AsyncWrite for Failing {
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

        let (mut outgoing, mut incoming) = mux(
            Failing,
            b"\0\0".as_slice(),
            BytesMut::default(),
            BytesMut::default(),
            [[None; 0]; 0],
        );
        let err = outgoing.write_all(b"\x2a").await.unwrap_err();
        assert_eq!(err.to_string(), "test");
        let err = incoming.read_to_end(&mut Vec::default()).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);

        let (mut tx, rx) = duplex(16);
        let (mut outgoing, incoming) = mux(
            sink(),
            rx,
            BytesMut::default(),
            BytesMut::default(),
            [[None; 0]; 0],
        );
        tx.write_all(&frame(&[1], b"\x2a")).await.unwrap();
        let err = incoming.closed().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let err = outgoing.write_all(b"\x2a").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        drop(incoming);
        let err = tx.write_all(b"\x2a").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
