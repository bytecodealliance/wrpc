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
//! [`Codec::serve`] *serves* WIT functions by receiving the parameters following the header of an
//! invocation and transmitting the results, [`serve()`] uses the default codecs. Generated
//! bindings, [`invoke()`] and [`serve()`] use any [`Invoke`] client and [`Serve`] server, such as
//! [`Framed`]. [`Invoke`] and [`Serve`] require [`Send`] values and futures, [`Codec::invoke`] and
//! [`Codec::serve`] do not.
//!
//! A transport establishes bidirectional byte streams, each carrying invocations one after
//! another, which it yields as any [`AsyncWrite`](tokio::io::AsyncWrite)/
//! [`AsyncRead`](tokio::io::AsyncRead) byte stream pair. [`transmit()`] and [`receive()`] layer
//! the wRPC [framing](FrameDecoder) protocol on top of it to multiplex the asynchronous values nested in
//! parameters and results, which encoders [Transmit] and decoders [Receive]. [`Listener`]
//! accepts the invocations sent over a byte stream pair one after another.
//!
//! Received asynchronous values are not buffered: until the consumer of a `future` or `stream`
//! takes the data received for it, nothing else of its invocation is received. Consume all of
//! them concurrently with each other and with the future receiving them, such as the [Receipt]
//! or the [Invocation], for example, with `join!` or by spawning a task draining each. If the peer
//! interleaves them, a consumer reading one `stream` to its end before polling another one stalls
//! the invocation indefinitely and without an error, unless the application ends it, for example,
//! with a timeout around the invocation; I/O timeouts do not fire, since nothing is read.
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
    FrameDecodeError, FrameDecoder, FrameEncoder, FrameHeaderDecoder, FrameHeaderEncoder,
    FrameHeaderRef, FrameRef,
};
pub use header::{Header, HeaderDecodeError, HeaderDecoder, HeaderEncoder, HeaderRef, PROTOCOL};
pub use invoke::{Invocation, Invoke, InvokeError, invoke};
pub use listener::Listener;
pub use mux::{Receipt, Transmission, receive, transmit};
pub use pack::{PackError, UnpackError, pack, pack_with, unpack, unpack_with};
pub use serve::{Serve, ServeError, serve};
pub use value::*;

use core::error::Error;
use core::fmt;

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
#[derive(Debug)]
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

/// Error of [receiving](Receive) a value
#[derive(Debug)]
pub enum ReceiveError {
    /// Data arrived for an asynchronous value whose consumer was dropped
    Unread,
    /// A frame arrived on a path, which is not pending
    UnknownPath,
    /// Data arrived on a path after its value ended or after the path was closed
    DataAfterEnd,
    /// The frames ended before all paths were closed or a path was closed without a value
    Truncated,
    /// A decoder rejected the data of a value, including a value cut short by its path closing
    Decode(std::io::Error),
    /// Reading the frames failed
    Frame(FrameDecodeError),
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unread => f.write_str("received data for a dropped consumer"),
            Self::UnknownPath => f.write_str("received frame on a path, which is not pending"),
            Self::DataAfterEnd => f.write_str("received data after value end"),
            Self::Truncated => f.write_str("value is incomplete"),
            Self::Decode(..) => f.write_str("failed to decode value"),
            Self::Frame(..) => f.write_str("failed to receive frame"),
        }
    }
}

impl Error for ReceiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unread | Self::UnknownPath | Self::DataAfterEnd | Self::Truncated => None,
            Self::Decode(err) => Some(err),
            Self::Frame(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for ReceiveError {
    fn from(err: std::io::Error) -> Self {
        Self::Decode(err)
    }
}

impl From<FrameDecodeError> for ReceiveError {
    fn from(err: FrameDecodeError) -> Self {
        Self::Frame(err)
    }
}

impl From<ReceiveError> for std::io::Error {
    fn from(err: ReceiveError) -> Self {
        match err {
            ReceiveError::Decode(err) => err,
            ReceiveError::Frame(err) => err.into(),
            ReceiveError::Unread => Self::new(std::io::ErrorKind::BrokenPipe, err),
            ReceiveError::Truncated => Self::new(std::io::ErrorKind::UnexpectedEof, err),
            ReceiveError::UnknownPath | ReceiveError::DataAfterEnd => {
                Self::new(std::io::ErrorKind::InvalidData, err)
            }
        }
    }
}
