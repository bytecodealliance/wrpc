use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::io::IoSlice;
use std::sync::{Arc, Mutex, PoisonError};

use futures::task::AtomicWaker;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Splits a stream into a [ReadHalf] and a [WriteHalf], calling `abort` on the stream before
/// closing it if the [WriteHalf] is dropped before being shut down.
pub(crate) fn split<T>(stream: T, abort: fn(&T)) -> (ReadHalf<T>, WriteHalf<T>) {
    let shared = Arc::new(Shared {
        stream: Mutex::new(Some(stream)),
        waker: AtomicWaker::default(),
        abort,
    });
    (
        ReadHalf {
            shared: Arc::clone(&shared),
            eof: false,
        },
        WriteHalf {
            shared,
            shutdown: false,
        },
    )
}

#[derive(Debug)]
struct Shared<T> {
    stream: Mutex<Option<T>>,
    waker: AtomicWaker,
    abort: fn(&T),
}

impl<T: Unpin> Shared<T> {
    fn poll<U>(
        &self,
        f: impl FnOnce(Pin<&mut T>) -> Poll<std::io::Result<U>>,
    ) -> Poll<std::io::Result<U>> {
        let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = stream.as_mut() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "write half dropped before being shut down",
            )));
        };
        f(Pin::new(stream))
    }
}

/// Read half of a stream, which fails once the [WriteHalf] is dropped before being shut down,
/// unless it already reached the end of the stream
#[derive(Debug)]
pub struct ReadHalf<T> {
    shared: Arc<Shared<T>>,
    eof: bool,
}

impl<T: AsyncRead + Unpin> AsyncRead for ReadHalf<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.eof {
            return Poll::Ready(Ok(()));
        }
        this.shared.waker.register(cx.waker());
        let n = buf.filled().len();
        ready!(this.shared.poll(|stream| stream.poll_read(cx, buf)))?;
        this.eof = buf.filled().len() == n && buf.remaining() > 0;
        Poll::Ready(Ok(()))
    }
}

/// Write half of a stream, which, if dropped before being shut down, closes the stream, signaling
/// the peer that sending data failed
#[derive(Debug)]
pub struct WriteHalf<T> {
    shared: Arc<Shared<T>>,
    shutdown: bool,
}

impl<T> Drop for WriteHalf<T> {
    fn drop(&mut self) {
        if self.shutdown {
            return;
        }
        let stream = self
            .shared
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stream) = stream {
            (self.shared.abort)(&stream);
        }
        self.shared.waker.wake();
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for WriteHalf<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.shared.poll(|stream| stream.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.shared
            .poll(|stream| stream.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        self.shared
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(AsyncWrite::is_write_vectored)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.shared.poll(|stream| stream.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        ready!(this.shared.poll(|stream| stream.poll_shutdown(cx)))?;
        this.shutdown = true;
        Poll::Ready(Ok(()))
    }
}
