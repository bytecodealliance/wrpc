//! wRPC WebTransport transport

use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use anyhow::Context as _;
use bytes::Bytes;
use tokio::io::{AsyncBufRead, AsyncRead, BufReader, ReadBuf};
use web_transport_quinn::{SendStream, Session};
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing, invoke};

/// WebTransport server, which aborts ingress once all handles are dropped
pub type Server = wrpc_transport::Server<(), RecvStream, SendStream, false>;

/// Buffered receive side of a WebTransport stream, which stops the stream with WebTransport
/// application error code `1` once dropped, signaling the peer that it is done receiving data
pub struct RecvStream(BufReader<web_transport_quinn::RecvStream>);

impl From<web_transport_quinn::RecvStream> for RecvStream {
    fn from(rx: web_transport_quinn::RecvStream) -> Self {
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

const DONE: u32 = 1;

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
            tx,
            RecvStream::from(rx),
            instance,
            func,
            params,
            paths,
            false,
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
