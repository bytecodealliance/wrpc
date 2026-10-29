//! wRPC QUIC transport

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use anyhow::Context as _;
use bytes::Bytes;
use quinn::{Connection, VarInt};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader, ReadBuf};
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing, invoke};

const DONE: VarInt = VarInt::from_u32(1);

const ABORTED: VarInt = VarInt::from_u32(0);

/// QUIC server
pub type Server = wrpc_transport::Server<(), RecvStream, SendStream>;

/// Buffered receive side of a QUIC stream, which stops the stream with application error code
/// `1` once dropped, signaling the peer that it is done receiving data
pub struct RecvStream(BufReader<quinn::RecvStream>);

impl From<quinn::RecvStream> for RecvStream {
    fn from(rx: quinn::RecvStream) -> Self {
        Self(BufReader::new(rx))
    }
}

impl Drop for RecvStream {
    fn drop(&mut self) {
        _ = self.0.get_mut().stop(DONE);
    }
}

impl AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncBufRead for RecvStream {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        Pin::new(&mut self.get_mut().0).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        Pin::new(&mut self.get_mut().0).consume(amt);
    }
}

/// Send side of a QUIC stream, which is reset with application error code `0` once
/// dropped before being shut down, signaling the peer that sending data failed
#[derive(Debug)]
pub struct SendStream {
    inner: quinn::SendStream,
    shutdown: bool,
}

impl From<quinn::SendStream> for SendStream {
    fn from(inner: quinn::SendStream) -> Self {
        Self {
            inner,
            shutdown: false,
        }
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        if !self.shutdown {
            _ = self.inner.reset(ABORTED);
        }
    }
}

impl AsyncWrite for SendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().inner), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().inner), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        ready!(AsyncWrite::poll_shutdown(Pin::new(&mut this.inner), cx))?;
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
        let (tx, rx) = self
            .0
            .open_bi()
            .await
            .context("failed to open parameter stream")?;
        invoke(
            SendStream::from(tx),
            RecvStream::from(rx),
            instance,
            func,
            params,
            paths,
        )
        .await
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
