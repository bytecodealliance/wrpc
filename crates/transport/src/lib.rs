#![allow(clippy::result_large_err, clippy::type_complexity)]
#![deny(missing_docs)]

//! wRPC transport abstractions, codec and framing
//!
//! wRPC is an RPC framework based on [WIT](https://component-model.bytecodealliance.org/design/wit.html).
//! It follows client-server model, where peers (servers) may serve function and method calls invoked by the other peers (clients).
//!
//! The two main abstractions on top of which wRPC is built are:
//! - [Invoke] - the client-side handle to a wRPC transport, allowing clients to *invoke* WIT functions over wRPC transport
//! - [Serve] - the server-side handle to a wRPC transport, allowing servers to *serve* WIT functions over wRPC transport
//!
//! [Invoke] and [Serve] establish a single bidirectional byte stream per invocation, which
//! they yield as any [`AsyncWrite`](tokio::io::AsyncWrite)/[`AsyncBufRead`](tokio::io::AsyncBufRead)
//! byte stream pair. [`InvokeExt`] and [`ServeExt`] layer the wRPC [framing](frame) protocol on
//! top of it to multiplex the nested async parameter and result sub-streams as the framed
//! [`frame::Outgoing`] and [`frame::Incoming`] streams.

pub mod frame;
pub mod invoke;
pub mod serve;

mod pack;
mod value;

pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, Frame, FrameRef, Server};
pub use invoke::{Invoke, InvokeExt};
pub use pack::{Packer, pack, pack_ready, unpack};
pub use serve::{Serve, ServeExt};
pub use value::*;

#[cfg(any(all(target_family = "wasm", feature = "wasi"), feature = "net"))]
pub use frame::tcp;
#[cfg(all(unix, feature = "net"))]
pub use frame::unix;

use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder as _;
use wasm_tokio::Leb128Encoder;

/// Multiplexes asynchronous value transmission using a structural `path`
pub trait Index: Sized {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> std::io::Result<Self>;
}

fn encode_ready_bytes_header(n: usize, dst: &mut BytesMut) -> std::io::Result<()> {
    let n = u32::try_from(n)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.reserve(1);
    dst.put_u8(0x01);
    Leb128Encoder.encode(n, dst)
}

fn copy_io_error(err: &std::io::Error) -> std::io::Error {
    std::io::Error::new(err.kind(), err.to_string())
}

/// Returns the error for data received after the end of a value
fn error_trailing_data() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "received data after value end",
    )
}

/// Returns the error for received data, which was not processed
fn error_unprocessed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "received data was not processed",
    )
}

/// Returns the error for data received for a closed receiver
fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream receiver closed")
}
