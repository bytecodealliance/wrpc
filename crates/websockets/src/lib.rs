//! wRPC WebSocket transport using [`tokio_websockets`]

use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::{Buf as _, Bytes};
use futures::{Sink as _, Stream as _};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, ReadBuf};
use tokio_websockets::{CloseCode, Message, WebSocketStream};

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "client")]
pub use client::Client;

pub use tokio_websockets;
#[cfg(feature = "client")]
#[doc(no_inline)]
pub use tokio_websockets::ClientBuilder;
#[cfg(feature = "server")]
#[doc(no_inline)]
pub use tokio_websockets::ServerBuilder;

fn error_closed_before_eof() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::ConnectionAborted,
        "WebSocket connection closed before end of transmit",
    )
}

struct Conn<T: AsyncRead + AsyncWrite + Unpin> {
    ws: WebSocketStream<T>,
    reader: Waker,
    eof_sent: bool,
    eof_received: bool,
    aborted: bool,
    unprocessed: bool,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Conn<T> {
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.ws)
            .poll_ready(cx)
            .map_err(std::io::Error::other)
    }

    fn start_send(&mut self, msg: Message) -> std::io::Result<()> {
        Pin::new(&mut self.ws)
            .start_send(msg)
            .map_err(std::io::Error::other)
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.ws)
            .poll_flush(cx)
            .map_err(std::io::Error::other)
    }

    fn poll_close(
        &mut self,
        cx: &mut Context<'_>,
        code: CloseCode,
        reason: &str,
    ) -> Poll<std::io::Result<()>> {
        ready!(self.poll_ready(cx))?;
        _ = self.start_send(Message::close(Some(code), reason));
        self.poll_flush(cx)
    }

    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<Option<Bytes>>> {
        if !self.eof_received {
            if self.aborted {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "write half dropped before being shut down",
                )));
            }
            self.reader.clone_from(cx.waker());
            loop {
                match ready!(Pin::new(&mut self.ws).poll_next(cx)) {
                    Some(Ok(msg)) if msg.is_text() => {
                        if !msg.as_payload().is_empty() {
                            return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "unexpected non-empty WebSocket text frame",
                            )));
                        }
                        break;
                    }
                    Some(Ok(msg)) if msg.is_close() => {
                        if msg.as_close().map(|(code, ..)| code) == Some(CloseCode::NORMAL_CLOSURE)
                        {
                            break;
                        }
                        return Poll::Ready(Err(error_closed_before_eof()));
                    }
                    Some(Ok(msg)) if msg.is_ping() || msg.is_pong() => {}
                    Some(Ok(msg)) => {
                        return Poll::Ready(Ok(Some(Bytes::from(msg.into_payload()))));
                    }
                    Some(Err(err)) => return Poll::Ready(Err(std::io::Error::other(err))),
                    None => return Poll::Ready(Err(error_closed_before_eof())),
                }
            }
            self.eof_received = true;
        }
        if self.eof_sent && !self.aborted {
            _ = ready!(self.poll_close(cx, CloseCode::NORMAL_CLOSURE, ""));
        }
        Poll::Ready(Ok(None))
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Drop for Conn<T> {
    fn drop(&mut self) {
        if self.aborted {
            return;
        }
        let cx = &mut Context::from_waker(Waker::noop());
        let processed = !self.unprocessed
            && loop {
                match Pin::new(&mut self.ws).poll_next(cx) {
                    Poll::Ready(Some(Ok(msg)))
                        if msg.is_close() || msg.is_ping() || msg.is_pong() => {}
                    Poll::Ready(Some(Ok(msg)))
                        if !self.eof_received && msg.is_text() && msg.as_payload().is_empty() =>
                    {
                        self.eof_received = true;
                    }
                    Poll::Ready(Some(Ok(..))) => break false,
                    Poll::Ready(Some(Err(..)) | None) | Poll::Pending => break true,
                }
            };
        _ = if processed {
            self.poll_close(cx, CloseCode::NORMAL_CLOSURE, "")
        } else {
            self.poll_close(cx, CloseCode::PROTOCOL_ERROR, "data was not processed")
        };
    }
}

type Shared<T> = Arc<Mutex<Conn<T>>>;

fn lock<T: AsyncRead + AsyncWrite + Unpin>(conn: &Shared<T>) -> MutexGuard<'_, Conn<T>> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Outgoing half of a split [`WebSocketStream`].
pub struct Outgoing<T: AsyncRead + AsyncWrite + Unpin> {
    conn: Shared<T>,
    shut_down: bool,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Drop for Outgoing<T> {
    fn drop(&mut self) {
        if self.shut_down {
            return;
        }
        let mut conn = lock(&self.conn);
        conn.aborted = true;
        conn.reader.wake_by_ref();
        _ = conn.poll_close(
            &mut Context::from_waker(Waker::noop()),
            CloseCode::GOING_AWAY,
            "writer dropped before shutdown",
        );
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for Outgoing<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut conn = lock(&self.conn);
        ready!(conn.poll_ready(cx))?;
        conn.start_send(Message::binary(Bytes::copy_from_slice(buf)))?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        lock(&self.conn).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut conn = lock(&this.conn);
        if conn.eof_received {
            ready!(conn.poll_close(cx, CloseCode::NORMAL_CLOSURE, ""))?;
        } else {
            if !conn.eof_sent {
                ready!(conn.poll_ready(cx))?;
                conn.start_send(Message::text(Bytes::new()))?;
                conn.eof_sent = true;
            }
            ready!(conn.poll_flush(cx))?;
        }
        this.shut_down = true;
        Poll::Ready(Ok(()))
    }
}

/// Incoming half of a split [`WebSocketStream`].
pub struct Incoming<T: AsyncRead + AsyncWrite + Unpin> {
    conn: Shared<T>,
    chunk: Bytes,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Drop for Incoming<T> {
    fn drop(&mut self) {
        if !self.chunk.is_empty() {
            lock(&self.conn).unprocessed = true;
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncBufRead for Incoming<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        let this = self.get_mut();
        while this.chunk.is_empty() {
            let Some(chunk) = ready!(lock(&this.conn).poll_next(cx))? else {
                break;
            };
            this.chunk = chunk;
        }
        Poll::Ready(Ok(&this.chunk))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.get_mut().chunk.advance(amt);
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for Incoming<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let chunk = ready!(self.as_mut().poll_fill_buf(cx))?;
        let n = chunk.len().min(buf.remaining());
        buf.put_slice(&chunk[..n]);
        self.consume(n);
        Poll::Ready(Ok(()))
    }
}

/// Splits a [`WebSocketStream`] into wRPC byte-stream halves: an [`AsyncWrite`]
/// outgoing half and an [`AsyncBufRead`] incoming half, suitable for use with
/// [`wrpc_transport::frame`].
///
/// Shutting down the outgoing half closes the connection with status code 1000 (Normal Closure)
/// if the incoming half already received the peer's end of transmit, and otherwise sends an EOF
/// sentinel, an empty text message, and closes the connection the same way once the incoming half
/// receives the peer's end of transmit, which is either an EOF sentinel or a close frame with
/// status code 1000.
///
/// Once both halves are dropped after the outgoing half was shut down, the connection is closed
/// normally, unless the incoming half left data unread or data, other than the peer's end of
/// transmit, was already received, in which case it is closed with an error.
/// If the outgoing half is dropped without being shut down, the incoming half fails and the
/// connection is closed with an error.
pub fn split<T>(ws: WebSocketStream<T>) -> (Outgoing<T>, Incoming<T>)
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let conn = Arc::new(Mutex::new(Conn {
        ws,
        reader: Waker::noop().clone(),
        eof_sent: false,
        eof_received: false,
        aborted: false,
        unprocessed: false,
    }));
    (
        Outgoing {
            conn: Arc::clone(&conn),
            shut_down: false,
        },
        Incoming {
            conn,
            chunk: Bytes::new(),
        },
    )
}
