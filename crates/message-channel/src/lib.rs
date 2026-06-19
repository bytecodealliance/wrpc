//! wRPC transport over a browser [`MessagePort`].
//!
//! A [`MessagePort`] (one half of a [`web_sys::MessageChannel`], or a port
//! transferred from a [`Worker`](web_sys::Worker), [`Window`](web_sys::Window),
//! etc.) is a single, already-established, bidirectional message stream. It
//! therefore maps onto a single wRPC connection, which the framed transport in
//! [`wrpc_transport::frame`] multiplexes into the sub-streams of one
//! invocation.
//!
//! [`connect`] bridges the event-based [`MessagePort`] API onto the
//! [`AsyncRead`]/[`AsyncWrite`] byte-stream halves the framed transport
//! expects. Because [`MessagePort`] (like all `web_sys` handles) is not [`Send`]
//! while [`Invoke`](wrpc_transport::Invoke)/[`Serve`](wrpc_transport::Serve)
//! require it, the port is moved into a task spawned with
//! [`wasm_bindgen_futures::spawn_local`] and is reached only through [`Send`]
//! channels; the returned halves are [`Send`] and satisfy the transport bounds.
//!
//! Each direction is framed as discrete messages: a [`Uint8Array`] carries a
//! chunk of bytes, and a `null` message is the end-of-stream sentinel.
//!
//! Like every wRPC transport, the framed layer drives its work on a Tokio
//! runtime; in the browser the caller is responsible for running one (e.g. a
//! current-thread runtime pumped on the event loop), just as native callers
//! supply `#[tokio::main]`.
//!
//! [`MessagePort`]: web_sys::MessagePort
#![cfg(target_family = "wasm")]

use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::Bytes;
use futures::channel::oneshot;
use futures::Sink;
use js_sys::Uint8Array;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::io::{SinkWriter, StreamReader};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, MessagePort};

#[doc(no_inline)]
pub use web_sys::MessagePort as Port;

/// A message queued for posting to the [`MessagePort`] by the relay task.
enum Out {
    Data(Bytes),
    Eof,
}

/// Outgoing half of a [`connect`]ed [`MessagePort`]: an [`AsyncWrite`] that
/// relays written buffers to the port.
///
/// [`AsyncWrite`]: tokio::io::AsyncWrite
pub type Outgoing = SinkWriter<OutgoingSink>;

/// Incoming half of a [`connect`]ed [`MessagePort`]: an [`AsyncRead`] fed by
/// messages received on the port.
///
/// [`AsyncRead`]: tokio::io::AsyncRead
pub type Incoming = StreamReader<UnboundedReceiverStream<std::io::Result<Bytes>>, Bytes>;

/// [`Invoke`](wrpc_transport::Invoke) implementation over a single
/// [`MessagePort`].
///
/// [`Invoke::invoke`](wrpc_transport::Invoke::invoke) can only be called at most
/// once, repeated calls will return an error.
pub type Client = wrpc_transport::frame::Oneshot<Incoming, Outgoing>;

/// [`Serve`](wrpc_transport::Serve) implementation over [`MessagePort`]
/// connections fed in via [`Server::accept`](wrpc_transport::frame::Server::accept).
pub type Server = wrpc_transport::frame::Server<(), Incoming, Outgoing>;

/// [`Sink`] forwarding written buffers to the [`MessagePort`] relay task.
pub struct OutgoingSink {
    tx: mpsc::UnboundedSender<Out>,
    eof_sent: bool,
}

fn broken_pipe() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "MessagePort relay was dropped",
    )
}

impl Sink<&[u8]> for OutgoingSink {
    type Error = std::io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, buf: &[u8]) -> Result<(), Self::Error> {
        self.get_mut()
            .tx
            .send(Out::Data(Bytes::copy_from_slice(buf)))
            .map_err(|_| broken_pipe())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if !this.eof_sent {
            this.tx.send(Out::Eof).map_err(|_| broken_pipe())?;
            this.eof_sent = true;
        }
        Poll::Ready(Ok(()))
    }
}

/// Bridges a [`MessagePort`] into wRPC byte-stream halves: an [`AsyncWrite`]
/// outgoing half and an [`AsyncRead`] incoming half, suitable for use with
/// [`wrpc_transport::frame`] (see [`Client`] and [`Server`]).
///
/// The port is moved into a [`spawn_local`] task that owns it for the lifetime
/// of the connection; the task closes the port once both halves have signalled
/// end-of-stream.
///
/// [`AsyncRead`]: tokio::io::AsyncRead
/// [`AsyncWrite`]: tokio::io::AsyncWrite
#[must_use]
pub fn connect(port: MessagePort) -> (Outgoing, Incoming) {
    let (in_tx, in_rx) = mpsc::unbounded_channel::<std::io::Result<Bytes>>();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Out>();
    let (in_done_tx, in_done_rx) = oneshot::channel::<()>();

    let mut in_tx = Some(in_tx);
    let mut in_done = Some(in_done_tx);
    let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        let data = ev.data();
        if data.is_null() || data.is_undefined() {
            // End-of-stream sentinel: dropping the sender signals EOF to the reader.
            in_tx = None;
            if let Some(done) = in_done.take() {
                let _ = done.send(());
            }
            return;
        }
        let item = match data.dyn_into::<Uint8Array>() {
            Ok(buf) => Ok(Bytes::from(buf.to_vec())),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected non-binary MessagePort message",
            )),
        };
        let err = item.is_err();
        if in_tx
            .as_ref()
            .is_none_or(|tx| tx.send(item).is_err() || err)
        {
            // Either the reader was dropped or we forwarded a terminal error;
            // tear the incoming half down either way.
            in_tx = None;
            if let Some(done) = in_done.take() {
                let _ = done.send(());
            }
        }
    });
    port.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    port.start();

    spawn_local(async move {
        // Keep the port and its message handler alive for the duration of the relay.
        let _onmessage = onmessage;
        while let Some(out) = out_rx.recv().await {
            let msg = match out {
                Out::Data(buf) => JsValue::from(Uint8Array::from(buf.as_ref())),
                Out::Eof => JsValue::NULL,
            };
            if port.post_message(&msg).is_err() {
                break;
            }
        }
        // Outgoing finished; wait for the peer to finish sending before closing.
        let _ = in_done_rx.await;
        port.close();
    });

    (
        SinkWriter::new(OutgoingSink {
            tx: out_tx,
            eof_sent: false,
        }),
        StreamReader::new(UnboundedReceiverStream::new(in_rx)),
    )
}

/// Constructs a [`Client`] over a single [`MessagePort`].
#[must_use]
pub fn client(port: MessagePort) -> Client {
    let (tx, rx) = connect(port);
    Client::from((rx, tx))
}
