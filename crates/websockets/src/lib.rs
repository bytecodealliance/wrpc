//! wRPC WebSocket transport

use core::iter;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use bytes::Bytes;
use futures::stream::SplitSink;
use futures::{Sink, SinkExt as _, StreamExt as _, TryStreamExt as _, future, stream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::{CopyToBytes, SinkWriter, StreamReader};
use tokio_websockets::{CloseCode, Error, Message, WebSocketStream};

/// Maximum length of a message sent by [`split`]
pub const MAX_MESSAGE_LEN: usize = 1 << 20;

struct Sender<T> {
    tx: SplitSink<WebSocketStream<T>, Message>,
    closing: bool,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Sink<Message> for Sender<T> {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.tx.poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Error> {
        self.tx.start_send_unpin(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.tx.poll_flush_unpin(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if !self.closing {
            ready!(self.tx.poll_ready_unpin(cx))?;
            self.tx
                .start_send_unpin(Message::close(Some(CloseCode::NORMAL_CLOSURE), ""))?;
            self.closing = true;
        }
        match ready!(self.tx.poll_flush_unpin(cx)) {
            Err(Error::AlreadyClosed) => self.tx.poll_flush_unpin(cx),
            res => Poll::Ready(res),
        }
    }
}

fn error_io(err: Error) -> std::io::Error {
    match err {
        Error::Io(err) => err,
        Error::PayloadTooLong { .. } | Error::Protocol(..) => {
            std::io::Error::new(std::io::ErrorKind::InvalidData, err)
        }
        err => std::io::Error::other(err),
    }
}

/// Splits `ws` into the byte streams of an invocation, written as binary messages of at most
/// [`MAX_MESSAGE_LEN`] bytes.
///
/// Received messages are bounded by the [`Limits`](tokio_websockets::Limits) of `ws`, peers using
/// [`split`] need no more than [`MAX_MESSAGE_LEN`].
///
/// Reading ends once the peer closes the connection with [`CloseCode::NORMAL_CLOSURE`] or ends it
/// without the closing handshake, and fails on any other close, a text message or a reset.
/// Shutting down the writer closes the connection with [`CloseCode::NORMAL_CLOSURE`], reading
/// continues until the peer acknowledges it. The underlying stream is closed once both halves are
/// dropped.
pub fn split(
    ws: WebSocketStream<impl AsyncRead + AsyncWrite + Unpin>,
) -> (impl AsyncWrite, impl AsyncRead) {
    let (tx, rx) = ws.split();
    let tx = Sender { tx, closing: false }
        .sink_map_err(error_io)
        .with_flat_map(|mut buf: Bytes| {
            stream::iter(iter::from_fn(move || {
                (!buf.is_empty()).then(|| {
                    Ok(Message::binary(
                        buf.split_to(buf.len().min(MAX_MESSAGE_LEN)),
                    ))
                })
            }))
        });
    let rx = rx.map_err(error_io).try_filter_map(|msg| {
        future::ready(if msg.is_binary() {
            Ok(Some(Bytes::from(msg.into_payload())))
        } else if msg.is_text() {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected WebSocket text message",
            ))
        } else if let Some((code, reason)) = msg.as_close()
            && code != CloseCode::NORMAL_CLOSURE
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!(
                    "WebSocket connection closed with code {}: {reason}",
                    u16::from(code)
                ),
            ))
        } else {
            Ok(None)
        })
    });
    (SinkWriter::new(CopyToBytes::new(tx)), StreamReader::new(rx))
}
