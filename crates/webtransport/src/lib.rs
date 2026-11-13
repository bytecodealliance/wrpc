//! wRPC WebTransport transport

use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use futures::{Stream, stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use wrpc_transport::Connect;

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

/// WebTransport session, which invokes over the bidirectional streams it opens and accepts
/// invocations over the bidirectional streams the peer opens
#[derive(Clone, Debug)]
pub struct Session(web_transport_quinn::Session);

impl From<web_transport_quinn::Session> for Session {
    fn from(session: web_transport_quinn::Session) -> Self {
        Self(session)
    }
}

impl Deref for Session {
    type Target = web_transport_quinn::Session;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Connect for Session {
    type Context = ();
    type Outgoing = SendStream;
    type Incoming = RecvStream;
    type Error = web_transport_quinn::SessionError;

    async fn connect(
        &self,
        (): Self::Context,
    ) -> Result<(Self::Outgoing, Self::Incoming), Self::Error> {
        let (tx, rx) = self.0.open_bi().await?;
        Ok((tx.into(), rx.into()))
    }
}

impl Session {
    /// Returns the stream of invocations accepted over the bidirectional streams the peer opens,
    /// which keeps a handle to the session and ends after the first error
    pub fn incoming(
        &self,
    ) -> impl Stream<Item = Result<((), SendStream, RecvStream), web_transport_quinn::SessionError>>
    + use<> {
        stream::unfold(Some(self.0.clone()), |session| async move {
            let session = session?;
            match session.accept_bi().await {
                Ok((tx, rx)) => Some((Ok(((), tx.into(), rx.into())), Some(session))),
                Err(err) => Some((Err(err), None)),
            }
        })
    }
}
