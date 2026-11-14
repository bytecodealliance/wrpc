use core::future::poll_fn;
use core::iter::zip;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};

use std::collections::BTreeMap;
use std::io::IoSlice;
use std::sync::Arc;

use bytes::{Buf, BufMut as _, Bytes, BytesMut};
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::select;
use tokio_util::codec::{Decoder, Encoder};
use tokio_util::io::poll_read_buf;
use tracing::{Instrument as _, debug, error, instrument, trace};
use wasm_tokio::{Leb128Encoder, put_u32_leb128};

use crate::frame::{FrameHeader, HeaderDecoder};
use crate::{Deferred, DeferredWrite, Encode, READ_CHUNK_SIZE};

/// Maximum number of bytes of encoded stream chunks buffered before they are written
const MAX_PENDING_STREAM_BATCH: usize = 1 << 20;

/// State of an invocation shared by its handles
struct IncomingState {
    stream: Pin<Box<dyn AsyncRead + Send + Unpin>>,
    /// Declared paths
    paths: Box<[Box<[Option<usize>]>]>,
    /// Decoder of the frame headers
    header: HeaderDecoder,
    /// Part of a frame header received
    header_buf: BytesMut,
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

impl IncomingState {
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
            let header = self.header.decode(&mut self.header_buf)?;
            if header.is_some() {
                break Ok(header);
            }
            let n = self.header.min_remaining();
            self.header_buf.reserve(n);
            let n = ready!(poll_read_buf(
                self.stream.as_mut(),
                cx,
                &mut (&mut self.header_buf).limit(n)
            ))?;
            if n == 0 {
                break self.header.decode_eof(&mut self.header_buf);
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

/// Locks `mutex`, recovering it if a thread panicked while holding the lock
fn lock<T: ?Sized>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Returns the root [Outgoing] handle of the framed egress of an invocation over `tx`, which
/// sends `buffer` before the first frame.
///
/// `tx` is polled by the handles, nothing is spawned: data is sent only while a handle writes,
/// flushes or closes. Every [Outgoing] handle must be closed with [`Outgoing::close`], the last
/// one shuts the stream down, which is otherwise dropped with the last handle without being shut
/// down. Every handle of a failed egress fails with [`std::io::ErrorKind::BrokenPipe`].
pub fn egress(tx: impl AsyncWrite + Send + Unpin + 'static, buffer: BytesMut) -> Outgoing {
    Outgoing {
        path: Box::default(),
        path_buf: Bytes::from_static(&[0]),
        closed: 0,
        state: Arc::new(std::sync::Mutex::new(OutgoingState {
            buffer,
            stream: Box::pin(tx) as _,
            pending: Vec::default(),
            failed: false,
            written: 0,
        })),
    }
}

/// Returns the root [Incoming] handle of the framed ingress of an invocation over `rx` declaring
/// the async `paths`.
///
/// `rx` is polled by the handles, nothing is spawned: data is received only while a handle reads
/// or the future returned by [`Incoming::closed`] is polled. A frame is received as a whole
/// before the next one and held until the handle of its path reads it, so all handles must be
/// read concurrently. Reads from `rx` are small, so they must be cheap, for example buffered.
/// The root handle and a handle, which read data of its path, must read the end of it before it
/// is dropped. Other handles of paths, which carry no value, such as the elements of a list of
/// futures, need not be read. The stream is dropped with the last handle and [Closed] future.
/// A handle of a declared path, which did not read data, is not known to expect any, since
/// merged variant paths may carry none, so a frame received on its path after it was dropped,
/// or on a closed path, is held like one, whose handle was not indexed yet, stalling ingress
/// until all handles are dropped.
/// Ingress fails on a frame received on a path, which is not declared, on a frame received on a
/// path no handle reads once all handles are dropped, on the stream ending before a handle read
/// the end of its path, on a handle dropped before it read the end of its path and on an error
/// of the stream. Every handle of a failed ingress fails with
/// [`std::io::ErrorKind::BrokenPipe`].
///
/// `paths` must not contain both a wildcard and an index at the same position, so paths of
/// different variant cases, which are not indexed, must be merged by replacing such indices
/// by wildcards.
pub fn ingress(
    rx: impl AsyncRead + Send + Unpin + 'static,
    paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
) -> Incoming {
    let paths: Box<[Box<[Option<usize>]>]> = paths
        .into_iter()
        .map(|path| Box::<[Option<usize>]>::from(path.as_ref()))
        .filter(|path| !path.is_empty())
        .collect();
    let depth = paths.iter().map(|path| path.len()).max().unwrap_or(0);
    let depth = u32::try_from(depth).unwrap_or(u32::MAX);
    Incoming {
        path: Box::default(),
        closed: false,
        buffer: BytesMut::default(),
        state: Arc::new(std::sync::Mutex::new(IncomingState {
            stream: Box::pin(rx) as _,
            paths,
            header: HeaderDecoder::new(depth, u32::MAX),
            header_buf: BytesMut::default(),
            frame: None,
            remaining: 0,
            ended: false,
            readers: BTreeMap::from([(Box::default(), None)]),
            closed: Vec::default(),
            incoming: 1,
            failed: false,
        })),
    }
}

/// Incoming framed stream
pub struct Incoming {
    path: Box<[usize]>,
    /// Whether this handle read the end of the data of its path, which the handle of a path,
    /// which is not declared, carries none of
    closed: bool,
    /// Data of the path received, but not decoded yet
    buffer: BytesMut,
    state: Arc<std::sync::Mutex<IncomingState>>,
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
        })
    }

    /// Returns a future, which receives ingress until the peer ends transmit and resolves to the
    /// result of the invocation once ingress ends
    #[must_use]
    pub fn closed(&self) -> Closed {
        Closed {
            state: Arc::clone(&self.state),
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
            let n = poll_fn(|cx| self.poll_read(cx)).await?;
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
        if self.buffer.is_empty() {
            let n = poll_fn(|cx| self.poll_read(cx)).await?;
            if n == 0 {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "received data after value end",
        ))
    }

    /// Receives data of the path into the buffer, returning the number of bytes received,
    /// which is 0 once the path is closed
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_read(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Ok(0));
        }
        let mut state = lock(&self.state);
        if state.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        match state.readers.get_mut(&self.path) {
            Some(reader) => *reader = None,
            None => {
                state.readers.insert(self.path.clone(), None);
            }
        }
        let res = 'read: {
            loop {
                let Some(frame) = &state.frame else {
                    if state.ended {
                        break 'read Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    match state.poll_frame(cx) {
                        Poll::Ready(Ok(())) => continue,
                        Poll::Ready(Err(err)) => break 'read Err(err),
                        Poll::Pending => break,
                    }
                };
                if **frame != *self.path {
                    trace!(?frame, "waiting for the frame of another path to be read");
                    break;
                }
                let n = if state.remaining == 0 {
                    trace!("closing path");
                    state.readers.remove(&self.path);
                    0
                } else {
                    self.buffer.reserve(state.remaining.min(READ_CHUNK_SIZE));
                    let remaining = u64::try_from(state.remaining).unwrap_or(u64::MAX);
                    let mut stream = state.stream.as_mut().take(remaining);
                    match poll_read_buf(Pin::new(&mut stream), cx, &mut self.buffer) {
                        Poll::Ready(Ok(0)) => {
                            break 'read Err(std::io::ErrorKind::UnexpectedEof.into());
                        }
                        Poll::Ready(Ok(n)) => n,
                        Poll::Ready(Err(err)) => break 'read Err(err),
                        Poll::Pending => break,
                    }
                };
                state.remaining = state.remaining.saturating_sub(n);
                if state.remaining == 0 {
                    state.frame = None;
                    state.wake();
                }
                break 'read Ok(n);
            }
            if let Some(reader) = state.readers.get_mut(&self.path) {
                *reader = Some(cx.waker().clone());
            }
            return Poll::Pending;
        };
        match res {
            Ok(n) => {
                self.closed = n == 0;
                Poll::Ready(Ok(n))
            }
            Err(err) => Poll::Ready(Err(state.fail_err(err))),
        }
    }
}

/// Future returned by [`Incoming::closed`]
pub struct Closed {
    state: Arc<std::sync::Mutex<IncomingState>>,
}

impl Drop for Closed {
    fn drop(&mut self) {
        lock(&self.state).wake();
    }
}

impl Future for Closed {
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.state);
        if state.failed {
            return Poll::Ready(Err(error_invocation_failed()));
        }
        if !state.ended
            && state.frame.is_none()
            && let Poll::Ready(Err(err)) = state.poll_frame(cx)
        {
            return Poll::Ready(Err(state.fail_err(err)));
        }
        if !state.ended {
            return pending(&mut state.closed, cx);
        }
        Poll::Ready(Ok(()))
    }
}

struct OutgoingState {
    buffer: BytesMut,
    stream: Pin<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: Vec<Waker>,
    failed: bool,
    written: usize,
}

impl OutgoingState {
    fn wake(&mut self) {
        for w in mem::take(&mut self.pending) {
            w.wake();
        }
    }
}

/// Outgoing framed stream.
///
/// After use, this must be closed via [`Outgoing::close`].
pub struct Outgoing {
    path: Box<[usize]>,
    path_buf: Bytes,
    closed: usize,
    state: Arc<std::sync::Mutex<OutgoingState>>,
}

impl Outgoing {
    /// Encodes `item` using `enc` and returns a future, which writes it, closes the path and
    /// transmits the values `item` deferred
    ///
    /// # Errors
    ///
    /// Returns an error if encoding `item` or indexing its deferred values fails
    #[instrument(level = "trace", skip_all, fields(path = ?self.path))]
    pub fn send<T, E>(
        mut self,
        mut enc: E,
        item: T,
    ) -> Result<impl Future<Output = std::io::Result<()>> + Send + 'static, E::Error>
    where
        E: Encoder<T> + Deferred<Self>,
        E::Error: From<std::io::Error>,
    {
        let mut buf = BytesMut::default();
        enc.encode(item, &mut buf)?;
        let deferred = enc.take_deferred().map(|f| f(&self, &[])).transpose()?;
        Ok(async move {
            self.write(&buf).await?;
            self.close().await?;
            if let Some(deferred) = deferred {
                deferred.await?;
            }
            Ok(())
        }
        .in_current_span())
    }

    /// Encodes `item` using `enc` and writes it as a frame, buffering the bytes the stream did
    /// not accept, which are written before the next frame of any path
    #[instrument(level = "trace", skip_all, fields(path = ?self.path))]
    pub async fn encode<T, E: Encoder<T>>(&mut self, enc: &mut E, item: T) -> Result<(), E::Error> {
        let mut buf = BytesMut::default();
        enc.encode(item, &mut buf)?;
        self.write(&buf).await?;
        Ok(())
    }

    /// Writes the buffered bytes and flushes the stream
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    pub async fn flush(&mut self) -> std::io::Result<()> {
        poll_fn(|cx| {
            let mut state = lock(&self.state);
            if state.failed {
                return Poll::Ready(Err(error_invocation_failed()));
            }
            self.poll_flush(&mut state, cx)
        })
        .await
    }

    /// Closes the path by writing an empty frame and drops the handle, shutting the stream down
    /// if it is the last handle
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    pub async fn close(mut self) -> std::io::Result<()> {
        poll_fn(|cx| {
            let mut state = lock(&self.state);
            if state.failed {
                return Poll::Ready(Err(error_invocation_failed()));
            }
            if self.closed == 0 {
                ready!(self.poll_write(&mut state, cx, &[]))?;
                self.closed = state.written.saturating_add(state.buffer.len());
            }
            if state.written < self.closed {
                ready!(self.poll_flush(&mut state, cx))?;
            }
            Poll::Ready(Ok(()))
        })
        .await?;
        let Some(state) = Arc::into_inner(self.state) else {
            return Ok(());
        };
        let OutgoingState { ref mut stream, .. } = state
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stream.shutdown().await
    }

    async fn write(&mut self, mut buf: &[u8]) -> std::io::Result<()> {
        while !buf.is_empty() {
            let n = poll_fn(|cx| {
                let mut state = lock(&self.state);
                if state.failed {
                    return Poll::Ready(Err(error_invocation_failed()));
                }
                self.poll_write(&mut state, cx, buf)
            })
            .await?;
            buf = &buf[n..];
        }
        Ok(())
    }

    fn poll_write(
        &self,
        state: &mut OutgoingState,
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
            state.failed = true;
        }
        Poll::Ready(res)
    }

    fn poll_flush(
        &self,
        state: &mut OutgoingState,
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
            state.failed = true;
        }
        Poll::Ready(res)
    }
}

impl Outgoing {
    /// Index the outgoing stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it.
    pub fn index(&self, path: &[usize]) -> std::io::Result<Self> {
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
        })
    }
}

impl DeferredWrite for Outgoing {
    fn write_future<T>(
        &self,
        path: &[usize],
        _offset: usize,
        item: impl Future<Output = std::io::Result<T>> + Send + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'static
    where
        T: Encode<Self>,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let w = self.index(path);
        async move {
            let mut w = w?;
            let item = item.await?;
            let mut enc = T::Encoder::default();
            let mut buf = BytesMut::default();
            enc.encode(item, &mut buf)?;
            w.write(&buf).await?;
            let nested = enc.take_deferred().map(|f| f(&w, &[0]));
            let nested = nested.transpose()?;
            w.close().await?;
            let Some(nested) = nested else {
                return Ok(());
            };
            nested.await
        }
    }

    fn write_stream<T>(
        &self,
        path: &[usize],
        _offset: usize,
        mut items: impl Stream<Item = Vec<T>> + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'static
    where
        T: Encode<Self> + Send + 'static,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let w = self.index(path);
        async move {
            let mut w = w?;
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
                                let fut = f(&w, &[i])?;
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
                            w.flush().await
                        } else {
                            trace!(?buf, "writing bytes from buffer");
                            w.write(&buf).await
                        }
                    }, if pending || !buf.is_empty() || (ended && !closed) => {
                        res?;
                        if buf.is_empty() {
                            pending = false;
                            closed = ended;
                        } else {
                            buf.clear();
                            pending = true;
                        }
                        tasks.extend(deferred.drain(..));
                    }
                }
            }
            w.close().await?;
            while let Some(res) = tasks.next().await {
                res?;
            }
            Ok(())
        }
    }

    fn write_stream_bytes(
        &self,
        path: &[usize],
        _offset: usize,
        mut items: impl Stream<Item = Bytes> + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'static {
        let w = self.index(path);
        async move {
            let mut w = w?;
            let mut buf = BytesMut::default();
            let mut pending = false;
            loop {
                select! {
                    chunk = items.next(), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                        let Some(chunk) = chunk else {
                            trace!("writing stream end");
                            w.write(&buf).await?;
                            return w.close().await
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
                    res = w.flush(), if pending || !buf.is_empty() => {
                        res?;
                        pending = false;
                        if !buf.is_empty() {
                            w.write(&buf).await?;
                            buf.clear();
                            pending = true;
                        }
                    }
                }
            }
        }
    }

    fn write_stream_read(
        &self,
        path: &[usize],
        _offset: usize,
        mut buf: BytesMut,
        mut items: impl AsyncRead + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'static {
        let w = self.index(path);
        async move {
            let mut w = w?;
            if !buf.is_empty() {
                let n = u32::try_from(buf.len())
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                trace!(n, "writing ready chunk length");
                w.encode(&mut Leb128Encoder, n).await?;
            }
            let mut chunk = BytesMut::with_capacity(READ_CHUNK_SIZE);
            let mut pending = false;
            loop {
                select! {
                    res = items.read_buf(&mut chunk), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                        let n = res?;
                        if n == 0 {
                            trace!("writing stream end");
                            w.write(&buf).await?;
                            return w.close().await
                        }
                        let n = u32::try_from(n).map_err(|err| {
                            std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                        })?;
                        trace!(n, "encoding chunk length");
                        Leb128Encoder.encode(n, &mut buf)?;
                        buf.extend_from_slice(&chunk);
                        chunk.clear();
                    }
                    res = w.flush(), if pending || !buf.is_empty() => {
                        res?;
                        pending = false;
                        if !buf.is_empty() {
                            w.write(&buf).await?;
                            buf.clear();
                            pending = true;
                        }
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
    use tokio_util::codec::{BytesCodec, FramedRead};
    use wasm_tokio::cm::U8Codec;

    use super::*;
    use crate::frame::{self, Frame, FrameRef};

    fn frame(path: &[usize], data: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::default();
        frame::Encoder
            .encode(FrameRef { path, data }, &mut buf)
            .unwrap();
        buf.to_vec()
    }

    async fn write(tx: &mut Outgoing, buf: &[u8]) -> std::io::Result<()> {
        tx.encode(&mut BytesCodec::default(), Bytes::copy_from_slice(buf))
            .await
    }

    fn mux(
        tx: impl AsyncWrite + Send + Unpin + 'static,
        rx: impl AsyncRead + Send + Unpin + 'static,
        buffer: BytesMut,
        paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
    ) -> (Outgoing, Incoming) {
        (egress(tx, buffer), ingress(rx, paths))
    }

    async fn read_to_end(rx: &mut Incoming) -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::default();
        while let Some(chunk) = rx.decode(&mut BytesCodec::default()).await? {
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }

    async fn incoming(
        paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
    ) -> (DuplexStream, Incoming, Closed) {
        let (tx, rx) = duplex(1024);
        let (root, rx) = mux(sink(), rx, BytesMut::default(), paths);
        root.close().await.unwrap();
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
        let (outgoing, mut rx) = mux(
            sink(),
            rx,
            BytesMut::default(),
            [&[Some(0)][..], &[Some(0), None, Some(0)]],
        );
        outgoing.close().await.unwrap();
        let closed = rx.closed();
        let Err(err) = rx.index(&[1]) else {
            panic!("indexing an undeclared path should fail");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let mut sub = rx.index(&[0]).unwrap();
        let mut nested = rx.index(&[0, 7, 0]).unwrap();
        let frames = [
            &root[..],
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
            async { read_to_end(&mut sub).await.unwrap() },
            async { read_to_end(&mut nested).await.unwrap() },
            async { read_to_end(&mut rx).await.unwrap() },
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
        let sub = tokio::spawn(async move { read_to_end(&mut sub).await.unwrap() });
        yield_now().await;
        let frames = [
            frame(&[0], b"\x2b"),
            frame(&[0], b""),
            frame(&[], b"\x2a"),
            frame(&[], b""),
        ]
        .concat();
        tx.write_all(&frames).await.unwrap();
        let buf = tokio::time::timeout(Duration::from_secs(5), read_to_end(&mut rx))
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
            let ((), res) = join!(
                async {
                    tx.write_all(&frames).await.unwrap();
                    drop(tx);
                },
                read_to_end(&mut rx),
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
        sub.decode(&mut U8Codec).await.unwrap();
        drop(sub);
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        drop(rx);

        let (mut tx, mut rx, closed) = incoming([[Some(0)]]).await;
        tx.write_all(&frame(&[], b"")).await.unwrap();
        read_to_end(&mut rx).await.unwrap();
        drop(rx);
        tx.write_all(&frame(&[0], b"")).await.unwrap();
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn outgoing_frames() {
        let (tx, mut rx) = duplex(4);
        let (mut outgoing, _incoming) =
            mux(tx, b"".as_slice(), BytesMut::default(), [[None; 0]; 0]);
        let mut sub = outgoing.index(&[1]).unwrap();
        let mut nested = sub.index(&[2]).unwrap();
        let data = b"\x2a".repeat(32);
        let ((), (), (), buf) = join!(
            async {
                write(&mut outgoing, b"\x2b").await.unwrap();
                outgoing.close().await.unwrap();
            },
            async {
                write(&mut sub, &data).await.unwrap();
                sub.close().await.unwrap();
            },
            async {
                write(&mut nested, b"\x2c").await.unwrap();
                nested.close().await.unwrap();
            },
            async {
                let mut buf = Vec::default();
                rx.read_to_end(&mut buf).await.unwrap();
                buf
            },
        );
        let frames: Vec<Frame> = FramedRead::new(buf.as_slice(), frame::Decoder::default())
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
                [[None; 0]; 0],
            );
            write(&mut outgoing, &data).await.unwrap();
            outgoing.close().await.unwrap();
            let tx = tx.lock().unwrap().clone();
            let frames: Vec<Frame> = FramedRead::new(tx.as_slice(), frame::Decoder::default())
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
        read_to_end(&mut rx).await.unwrap();
        let err = read_to_end(&mut sub).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        let err = wait(closed).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test_log::test(tokio::test)]
    async fn final_flush() {
        let (tx, mut rx) = duplex(1);
        let (outgoing, _incoming) = mux(tx, b"".as_slice(), BytesMut::default(), [[None; 0]; 0]);
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

        let mut outgoing = egress(Failing, BytesMut::default());
        let err = write(&mut outgoing, b"\x2a").await.unwrap_err();
        assert_eq!(err.to_string(), "test");
        let err = write(&mut outgoing, b"\x2a").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);

        let (mut tx, rx) = duplex(16);
        let incoming = ingress(rx, [[None; 0]; 0]);
        tx.write_all(&frame(&[1], b"\x2a")).await.unwrap();
        let err = incoming.closed().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        drop(incoming);
        let err = tx.write_all(b"\x2a").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
