//! wRPC WebSocket transport using [tokio_websockets]

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use anyhow::Context as _;
use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{Sink, Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::{SinkWriter, StreamReader};
use tokio_websockets::resolver::{Gai, Resolver};
use tokio_websockets::{Message, WebSocketStream};
use tracing::instrument;

use wrpc_transport::Invoke;
use wrpc_transport::frame::invoke;

pub use tokio_websockets;
#[doc(no_inline)]
pub use tokio_websockets::{ClientBuilder, ServerBuilder};

/// Outgoing half of a split [`WebSocketStream`].
pub struct Outgoing<T> {
    sink: SplitSink<WebSocketStream<T>, Message>,
    eof_sent: bool,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Sink<&[u8]> for Outgoing<T> {
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink)
            .poll_ready(cx)
            .map_err(std::io::Error::other)
    }

    fn start_send(mut self: Pin<&mut Self>, buf: &[u8]) -> Result<(), Self::Error> {
        let buf = Message::binary(Bytes::copy_from_slice(buf));
        Pin::new(&mut self.sink)
            .start_send(buf)
            .map_err(std::io::Error::other)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink)
            .poll_flush(cx)
            .map_err(std::io::Error::other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.as_mut().get_mut();
        if !this.eof_sent {
            ready!(Pin::new(&mut this.sink).poll_ready(cx)).map_err(std::io::Error::other)?;
            Pin::new(&mut this.sink)
                .start_send(Message::text(Bytes::new()))
                .map_err(std::io::Error::other)?;
            this.eof_sent = true;
        }
        Pin::new(&mut this.sink)
            .poll_flush(cx)
            .map_err(std::io::Error::other)
    }
}

/// Incoming half of a split [`WebSocketStream`].
#[repr(transparent)]
pub struct Incoming<T>(SplitStream<WebSocketStream<T>>);

impl<T: AsyncRead + AsyncWrite + Unpin> Stream for Incoming<T> {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(Pin::new(&mut self.0).poll_next(cx)) {
                Some(Ok(msg)) if msg.is_text() => {
                    if msg.as_payload().is_empty() {
                        return Poll::Ready(None);
                    }
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "unexpected non-empty WebSocket text frame",
                    ))));
                }
                Some(Ok(msg)) if msg.is_close() => {
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "WebSocket connection closed before EOF sentinel",
                    ))));
                }
                Some(Ok(msg)) if msg.is_ping() || msg.is_pong() => continue,
                Some(Ok(msg)) => return Poll::Ready(Some(Ok(Bytes::from(msg.into_payload())))),
                Some(Err(err)) => return Poll::Ready(Some(Err(std::io::Error::other(err)))),
                None => return Poll::Ready(None),
            }
        }
    }
}

/// Splits a [`WebSocketStream`] into wRPC byte-stream halves: an [`AsyncWrite`]
/// outgoing half and an [`AsyncRead`] incoming half, suitable for use with
/// [`wrpc_transport::frame`].
pub fn split<T>(
    ws: WebSocketStream<T>,
) -> (SinkWriter<Outgoing<T>>, StreamReader<Incoming<T>, Bytes>)
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (tx, rx) = ws.split();
    (
        SinkWriter::new(Outgoing {
            sink: tx,
            eof_sent: false,
        }),
        StreamReader::new(Incoming(rx)),
    )
}

/// [Invoke] implementation in terms of a [`ClientBuilder`].
#[repr(transparent)]
pub struct Client<'a, R: Resolver = Gai>(ClientBuilder<'a, R>);

impl<'a, R: Resolver> Client<'a, R> {
    /// Constructs a new [Client] from a preconfigured [`ClientBuilder`].
    #[must_use]
    pub fn from_builder(builder: ClientBuilder<'a, R>) -> Self {
        Self(builder)
    }
}

impl<'a, R: Resolver> From<ClientBuilder<'a, R>> for Client<'a, R> {
    fn from(builder: ClientBuilder<'a, R>) -> Self {
        Self::from_builder(builder)
    }
}

impl<R: Resolver + Send + Sync> Invoke for Client<'_, R> {
    type Context = ();
    type Outgoing = wrpc_transport::frame::Outgoing;
    type Incoming = wrpc_transport::frame::Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
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
        let (ws, _res) = self
            .0
            .connect()
            .await
            .context("failed to establish WebSocket connection")?;
        let (tx, rx) = split(ws);
        invoke(tx, rx, instance, func, params, paths).await
    }
}
