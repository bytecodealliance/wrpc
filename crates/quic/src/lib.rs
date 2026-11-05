#![allow(clippy::result_large_err)]

//! wRPC QUIC transport

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use anyhow::Context as _;
use bytes::Bytes;
use quinn::{Connection, VarInt};
use tokio::io::{AsyncRead, AsyncWrite, BufReader, ReadBuf};
use wrpc_transport::Invoke;
use wrpc_transport::frame::{AcceptError, invoke};

/// Error code of a stream, which was stopped or reset, because the invocation failed
const FAILED: VarInt = VarInt::from_u32(1);

/// [`quinn::RecvStream`], which is stopped once dropped before reaching the end of the stream,
/// since that fails the invocation
#[derive(Debug)]
pub struct RecvStream {
    stream: quinn::RecvStream,
    /// Whether the end of the stream was reached
    eof: bool,
}

impl From<quinn::RecvStream> for RecvStream {
    fn from(stream: quinn::RecvStream) -> Self {
        Self { stream, eof: false }
    }
}

impl Drop for RecvStream {
    fn drop(&mut self) {
        if !self.eof {
            _ = self.stream.stop(FAILED);
        }
    }
}

impl AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let this = self.get_mut();
        let n = buf.filled().len();
        ready!(Pin::new(&mut this.stream).poll_read(cx, buf))?;
        if buf.filled().len() == n {
            this.eof = true;
        }
        Poll::Ready(Ok(()))
    }
}

/// [`quinn::SendStream`], which is reset once dropped before being shut down, since that fails
/// the invocation
#[derive(Debug)]
pub struct SendStream {
    stream: quinn::SendStream,
    /// Whether the stream was shut down
    finished: bool,
}

impl From<quinn::SendStream> for SendStream {
    fn from(stream: quinn::SendStream) -> Self {
        Self {
            stream,
            finished: false,
        }
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        if !self.finished {
            _ = self.stream.reset(FAILED);
        }
    }
}

impl AsyncWrite for SendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().stream), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        ready!(Pin::new(&mut this.stream).poll_shutdown(cx))?;
        this.finished = true;
        Poll::Ready(Ok(()))
    }
}

/// QUIC server
pub type Server = wrpc_transport::Server<(), BufReader<RecvStream>, SendStream>;

/// Accepts an invocation on a bidirectional stream, see [`Server::accept`].
///
/// # Errors
///
/// Returns an error if accepting the invocation fails
pub async fn accept(
    srv: &Server,
    tx: quinn::SendStream,
    rx: quinn::RecvStream,
) -> Result<(), AcceptError<(), BufReader<RecvStream>, SendStream>> {
    srv.accept((), tx.into(), BufReader::new(rx.into())).await
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
    type Outgoing = SendStream;
    type Incoming = BufReader<RecvStream>;

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let (tx, rx) = self
            .0
            .open_bi()
            .await
            .context("failed to open parameter stream")?;
        invoke(tx.into(), BufReader::new(rx.into()), instance, func, params).await
    }
}

impl Invoke for Client {
    type Context = ();
    type Outgoing = SendStream;
    type Incoming = BufReader<RecvStream>;

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        (&self).invoke((), instance, func, params, paths).await
    }
}
