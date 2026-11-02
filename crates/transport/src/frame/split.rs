use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};

use std::io::IoSlice;
use std::sync::{Arc, Mutex, PoisonError};

use futures::task::AtomicWaker;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::frame::Reject;

/// Splits a stream into a [ReadHalf] and a [WriteHalf].
///
/// Dropping the [WriteHalf] shuts down the write side of the stream. Rejecting the [ReadHalf]
/// closes the stream. Transports, which can reset the stream, provide their own `split`, e.g.
/// `tcp::split`.
pub fn split<T: AsyncWrite + Unpin>(stream: T) -> (ReadHalf<T>, WriteHalf<T>) {
    split_with(stream, |_| {})
}

/// Splits a stream into a [ReadHalf] and a [WriteHalf], calling `abort` on the stream before
/// closing it if the [ReadHalf] is rejected.
pub(crate) fn split_with<T: AsyncWrite + Unpin>(
    stream: T,
    abort: fn(&T),
) -> (ReadHalf<T>, WriteHalf<T>) {
    let shared = Arc::new(Shared {
        stream: Mutex::new(Some(stream)),
        writer: AtomicWaker::default(),
        abort,
        shutdown: |stream| {
            _ = Pin::new(stream).poll_shutdown(&mut Context::from_waker(Waker::noop()));
        },
    });
    (
        ReadHalf {
            shared: Arc::clone(&shared),
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
    writer: AtomicWaker,
    abort: fn(&T),
    shutdown: fn(&mut T),
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
                "stream aborted",
            )));
        };
        f(Pin::new(stream))
    }
}

/// Read half of a stream. Rejecting it closes the stream.
#[derive(Debug)]
pub struct ReadHalf<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Reject for ReadHalf<T> {
    fn reject(self) {
        let stream = self
            .shared
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stream) = stream {
            (self.shared.abort)(&stream);
        }
        self.shared.writer.wake();
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for ReadHalf<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.shared.poll(|stream| stream.poll_read(cx, buf))
    }
}

/// Write half of a stream, which shuts down the write side of the stream once dropped.
/// It fails once the [ReadHalf] is rejected.
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
        let mut stream = self
            .shared
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = stream.as_mut() {
            (self.shared.shutdown)(stream);
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for WriteHalf<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.shared.writer.register(cx.waker());
        self.shared.poll(|stream| stream.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.shared.writer.register(cx.waker());
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
        self.shared.writer.register(cx.waker());
        self.shared.poll(|stream| stream.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.shared.writer.register(cx.waker());
        ready!(this.shared.poll(|stream| stream.poll_shutdown(cx)))?;
        this.shutdown = true;
        Poll::Ready(Ok(()))
    }
}
