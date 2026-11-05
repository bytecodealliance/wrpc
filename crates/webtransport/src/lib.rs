#![allow(clippy::result_large_err)]

//! wRPC WebTransport transport

use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use anyhow::Context as _;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, BufReader, ReadBuf};
use web_transport_quinn::Session;
use wrpc_transport::Invoke;
use wrpc_transport::frame::{AcceptError, invoke};

/// Error code of a stream, which was stopped or reset, because the invocation failed
const FAILED: u32 = 1;

/// [`web_transport_quinn::RecvStream`], which is stopped once dropped before reaching the end
/// of the stream, since that fails the invocation
#[derive(Debug)]
pub struct RecvStream {
    stream: web_transport_quinn::RecvStream,
    /// Whether the end of the stream was reached
    eof: bool,
}

impl From<web_transport_quinn::RecvStream> for RecvStream {
    fn from(stream: web_transport_quinn::RecvStream) -> Self {
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

/// [`web_transport_quinn::SendStream`], which is reset once dropped before being shut down,
/// since that fails the invocation
#[derive(Debug)]
pub struct SendStream {
    stream: web_transport_quinn::SendStream,
    /// Whether the stream was shut down
    finished: bool,
}

impl From<web_transport_quinn::SendStream> for SendStream {
    fn from(stream: web_transport_quinn::SendStream) -> Self {
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

/// WebTransport server
pub type Server = wrpc_transport::Server<(), BufReader<RecvStream>, SendStream>;

/// Accepts an invocation on a bidirectional stream, see [`Server::accept`].
///
/// # Errors
///
/// Returns an error if accepting the invocation fails
pub async fn accept(
    srv: &Server,
    tx: web_transport_quinn::SendStream,
    rx: web_transport_quinn::RecvStream,
) -> Result<(), AcceptError<(), BufReader<RecvStream>, SendStream>> {
    srv.accept((), tx.into(), BufReader::new(rx.into())).await
}

/// WebTransport wRPC client
#[derive(Clone, Debug)]
pub struct Client(Session);

impl From<Session> for Client {
    fn from(session: Session) -> Self {
        Self(session)
    }
}

impl Deref for Client {
    type Target = Session;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
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
