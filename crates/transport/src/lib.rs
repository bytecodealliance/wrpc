#![allow(clippy::type_complexity)]
#![deny(missing_docs)]

//! wRPC transport abstractions, codec and framing
//!
//! wRPC is an RPC framework based on
//! [WIT](https://component-model.bytecodealliance.org/design/wit.html). It follows client-server
//! model, where peers (servers) may serve function and method calls invoked by the other peers
//! (clients).
//!
//! The two main abstractions on top of which wRPC is built are:
//! - [`transmit()`] - sends a value, the parameters when invoking and the results when serving
//! - [`receive()`] - receives a value, the results when invoking and the parameters when serving
//!
//! [`Codec::invoke`] *invokes* WIT functions by sending the invocation header and transmitting
//! the parameters while receiving the results, [`invoke()`] uses the default codecs.
//! [`Server::serve`] *serves* WIT functions by receiving the parameters of an invocation, which
//! a [`Listener`] accepted, and transmitting the results, [`serve()`] uses the default codecs.
//! Generated bindings, [`invoke()`] and [`serve()`] use any [`Invoke`] client, such as [`Framed`],
//! and [`Serve`] server, such as [`Server`]. [`Serve`] requires [`Send`] values and futures,
//! [`Server::serve`] does not.
//!
//! A transport establishes bidirectional byte streams, each carrying invocations one after
//! another, which it yields as any [`AsyncWrite`](tokio::io::AsyncWrite)/
//! [`AsyncRead`](tokio::io::AsyncRead) byte stream pair. [`transmit()`] and [`receive()`] layer
//! the wRPC [framing](FrameDecoder) protocol on top of it to multiplex the asynchronous values nested in
//! parameters and results, which encoders [Transmit] and decoders [Receive]. [`Listener`]
//! accepts the invocations sent over a byte stream pair one after another, serving them
//! concurrently pipelines them.
//!
//! Over TCP, enable `TCP_NODELAY`, since invocations consist of small writes, each awaited by
//! the peer.

mod frame;

mod header;
mod invoke;
mod listener;
mod mux;
mod pack;
mod serve;
mod value;

pub use frame::{
    FrameDecoder, FrameEncoder, FrameHeaderDecoder, FrameHeaderEncoder, FrameHeaderRef, FrameRef,
};
pub use header::{Header, HeaderDecoder, HeaderEncoder, HeaderRef, PROTOCOL};
pub use invoke::{Invocation, Invoke, invoke, invoke_blocking};
pub use listener::Listener;
pub use mux::{Receipt, Transmission, receive, transmit};
pub use pack::{pack, unpack};
pub use serve::{Serve, Server, serve};
pub use value::*;

use bytes::{Buf, BufMut as _, BytesMut};
use tokio_util::codec::{Encoder as _, FramedRead, FramedWrite};

/// Codec of the values of invocations
#[derive(Clone, Copy, Debug, Default)]
pub struct Codec<E, D> {
    /// Encoder of the values sent, the parameters when invoking and the results when serving
    pub enc: E,
    /// Decoder of the values received, the results when invoking and the parameters when serving
    pub dec: D,
}

/// Frames written to and read from a byte stream pair
pub struct Framed<I, O> {
    /// Writer of the frames sent
    pub tx: FramedWrite<O, FrameEncoder>,
    /// Reader of the frames received
    pub rx: FramedRead<I, FrameDecoder>,
}

impl<I, O> Framed<I, O> {
    /// Constructs a new [Framed] over the byte streams `tx` and `rx`
    pub fn new(tx: O, rx: I) -> Self {
        Self {
            tx: FramedWrite::new(tx, FrameEncoder),
            rx: FramedRead::new(rx, FrameDecoder::default()),
        }
    }
}

/// Encodes `data` as frames on `path` into `dst`, encoding nothing if `data` is empty
fn encode_frames(path: &[usize], mut data: impl Buf, dst: &mut BytesMut) -> std::io::Result<()> {
    while data.has_remaining() {
        let size = data
            .remaining()
            .min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
        FrameHeaderEncoder.encode(FrameHeaderRef { path, size }, dst)?;
        dst.put((&mut data).take(size));
    }
    Ok(())
}

/// Encodes the frame closing `path` into `dst`
fn encode_close(path: &[usize], dst: &mut BytesMut) -> std::io::Result<()> {
    FrameEncoder.encode(FrameRef { path, data: &[] }, dst)
}

/// Returns the error for a frame received on `path`, which is not pending
pub fn error_unknown_path(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("received frame on unknown path `{path:?}`"),
    )
}

/// Returns the error for data received after the end of a value
pub fn error_data_after_end() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "received data after value end",
    )
}

/// Returns the error for a connection, on which receiving a header or a previous invocation failed
fn error_connection_failed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "connection failed")
}

/// Returns the error for an invocation, whose writer was lost by a previous invocation failing
fn error_writer_lost() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "writer lost")
}

/// Returns the error for data received for a closed receiver
fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed")
}
