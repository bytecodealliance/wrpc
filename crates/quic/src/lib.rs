//! wRPC QUIC transport

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context as _;
use bytes::Bytes;
use futures::task::AtomicWaker;
use quinn::{Connection, VarInt};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader, ReadBuf};
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing, Reject, invoke};

const DONE: VarInt = VarInt::from_u32(1);

const ABORTED: VarInt = VarInt::from_u32(0);

/// QUIC server
pub type Server = wrpc_transport::Server<(), RecvStream, SendStream>;

/// Splits a bidirectional QUIC stream into a [`RecvStream`] and a [`SendStream`].
#[must_use]
pub fn split((tx, rx): (quinn::SendStream, quinn::RecvStream)) -> (RecvStream, SendStream) {
    let shared = Arc::new(Shared {
        tx: Mutex::new(Some(tx)),
        waker: AtomicWaker::default(),
    });
    (
        RecvStream {
            rx: BufReader::new(rx),
            shared: Arc::clone(&shared),
        },
        SendStream {
            shared,
            shutdown: false,
        },
    )
}

#[derive(Debug)]
struct Shared {
    tx: Mutex<Option<quinn::SendStream>>,
    waker: AtomicWaker,
}

impl Shared {
    fn reset(&self) {
        let tx = self
            .tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut tx) = tx {
            _ = tx.reset(ABORTED);
        }
        self.waker.wake();
    }

    fn poll<T>(
        &self,
        f: impl FnOnce(Pin<&mut quinn::SendStream>) -> Poll<std::io::Result<T>>,
    ) -> Poll<std::io::Result<T>> {
        let mut tx = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(tx) = tx.as_mut() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "stream reset",
            )));
        };
        f(Pin::new(tx))
    }
}

/// Buffered receive side of a QUIC stream, which stops the stream with application error code
/// `1` once dropped, signaling the peer that it is done receiving data.
/// Rejecting it stops the stream and resets the [`SendStream`] with application error code `0`.
pub struct RecvStream {
    rx: BufReader<quinn::RecvStream>,
    shared: Arc<Shared>,
}

impl Drop for RecvStream {
    fn drop(&mut self) {
        _ = self.rx.get_mut().stop(DONE);
    }
}

impl Reject for RecvStream {
    fn reject(mut self) {
        _ = self.rx.get_mut().stop(ABORTED);
        self.shared.reset();
    }
}

impl AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().rx).poll_read(cx, buf)
    }
}

impl AsyncBufRead for RecvStream {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        Pin::new(&mut self.get_mut().rx).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        Pin::new(&mut self.get_mut().rx).consume(amt);
    }
}

/// Send side of a QUIC stream, which is reset with application error code `0` once
/// dropped before being shut down or once the [`RecvStream`] is rejected
#[derive(Debug)]
pub struct SendStream {
    shared: Arc<Shared>,
    shutdown: bool,
}

impl Drop for SendStream {
    fn drop(&mut self) {
        if !self.shutdown {
            self.shared.reset();
        }
    }
}

impl AsyncWrite for SendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.shared.waker.register(cx.waker());
        self.shared.poll(|tx| AsyncWrite::poll_write(tx, cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.shared.waker.register(cx.waker());
        self.shared.poll(|tx| AsyncWrite::poll_flush(tx, cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.shared.waker.register(cx.waker());
        ready!(this.shared.poll(|tx| AsyncWrite::poll_shutdown(tx, cx)))?;
        this.shutdown = true;
        Poll::Ready(Ok(()))
    }
}

/// QUIC wRPC client
#[derive(Clone, Debug)]
pub struct Client(Connection);

impl From<Connection> for Client {
    fn from(conn: Connection) -> Self {
        Self(conn)
    }
}

impl Invoke for &Client {
    type Context = ();

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = self
            .0
            .open_bi()
            .await
            .context("failed to open parameter stream")?;
        let (rx, tx) = split(stream);
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Invoke for Client {
    type Context = ();

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        (&self).invoke((), instance, func, params, paths).await
    }
}
