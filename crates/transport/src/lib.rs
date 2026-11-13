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
//! - [`invoke()`] - the client side, which *invokes* WIT functions over the byte streams of an
//!   invocation, for example ones established by a [Connect] implementation
//! - [`serve()`] - the server side, which *serves* WIT functions over the byte streams of an
//!   invocation, whose header was read by [`Header::read`], for example ones accepted by a transport
//!
//! A transport establishes a single bidirectional byte stream per invocation, which it yields as
//! any [`AsyncWrite`](tokio::io::AsyncWrite)/[`AsyncRead`](tokio::io::AsyncRead) byte
//! stream pair. [`invoke()`] and [`serve()`] layer the wRPC [framing](frame) protocol on top of
//! it to multiplex the nested async parameter and result sub-streams as the framed
//! [`frame::Outgoing`] and [`frame::Incoming`] streams.

pub mod frame;
#[cfg(any(all(target_family = "wasm", feature = "wasi"), feature = "net"))]
pub mod tcp;
#[cfg(all(unix, feature = "net"))]
pub mod unix;

mod connect;
mod header;
mod invoke;
mod mux;
mod oneshot;
mod pack;
mod serve;
mod value;

pub use connect::{Connect, ConnectExt, Timeout, TimeoutError};
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, Frame, FrameRef};
pub use header::{Header, HeaderDecoder, HeaderEncoder, HeaderReadError, HeaderRef, PROTOCOL};
pub use invoke::{invoke, invoke_blocking};
pub use oneshot::Oneshot;
pub use pack::{Packer, pack, pack_ready, unpack};
pub use serve::serve;
pub use value::*;

use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder as _;
use wasm_tokio::Leb128Encoder;

/// Default maximum length of the instance and function names in a [Header]
const DEFAULT_MAX_NAME_LEN: u32 = 1 << 10;

fn encode_ready_bytes_header(n: usize, dst: &mut BytesMut) -> std::io::Result<()> {
    let n = u32::try_from(n)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.reserve(1);
    dst.put_u8(0x01);
    Leb128Encoder.encode(n, dst)
}

/// Returns the error for data received for a closed receiver
fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream receiver closed")
}
