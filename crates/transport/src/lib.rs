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
//!   invocation
//! - [`serve()`] - the server side, which *serves* WIT functions over the frames following the
//!   header of an invocation
//!
//! A transport establishes bidirectional byte streams, each carrying invocations one after
//! another, which it yields as any [`AsyncWrite`](tokio::io::AsyncWrite)/
//! [`AsyncRead`](tokio::io::AsyncRead) byte stream pair. [`invoke()`] and [`serve()`] layer the
//! wRPC [framing](frame) protocol on top of it to multiplex the asynchronous values nested in
//! parameters and results, which encoders [Transmit] and decoders [Receive]. [`Listener`]
//! accepts the invocations sent over a byte stream pair one after another.
//!
//! Over TCP, enable `TCP_NODELAY`, since invocations consist of small writes, each awaited by
//! the peer.

pub mod frame;
#[cfg(all(target_family = "wasm", feature = "wasi"))]
pub mod tcp;

mod header;
mod invoke;
mod listener;
mod mux;
mod pack;
mod serve;
mod value;

pub use frame::{
    Decoder as FrameDecoder, Encoder as FrameEncoder, FrameHeaderDecoder, FrameHeaderEncoder,
    FrameHeaderRef, FrameRef,
};
pub use header::{Header, HeaderDecoder, HeaderEncoder, HeaderReadError, HeaderRef, PROTOCOL};
pub use invoke::{Invocation, invoke, invoke_blocking};
pub use listener::Listener;
pub use pack::{pack, unpack};
/// Awaiting the futures returned by [`invoke()`] and [`serve()`] in a [`Send`] future may fail to
/// compile with "one type is more general than the other" or "implementation of ... is not
/// general enough" due to [rust-lang/rust#96865](https://github.com/rust-lang/rust/issues/96865),
/// call [`send`](SendFuture::send) on them before awaiting to work around it:
///
/// ```
/// use futures::stream::BoxStream;
/// use tokio::io::{AsyncRead, AsyncWrite};
/// use wrpc_transport::{Decode, Encode, SendFuture as _, invoke};
///
/// fn invoke_send(
///     tx: impl AsyncWrite + Send + Unpin,
///     rx: impl AsyncRead + Send + Unpin,
/// ) -> impl Future<Output = std::io::Result<BoxStream<'static, Vec<String>>>> + Send {
///     async {
///         let ((s,), ..) = invoke(
///             tx,
///             rx,
///             "wrpc-test:integration/async",
///             "with-streams",
///             <() as Encode>::Encoder::default(),
///             (),
///             <(BoxStream<'static, Vec<String>>,) as Decode>::Decoder::default(),
///         )
///         .send()
///         .await?;
///         Ok(s)
///     }
/// }
/// ```
pub use send_future::SendFuture;
pub use serve::serve;
pub use value::*;

use bytes::{Buf, BufMut as _, BytesMut};
use tokio_util::codec::Encoder as _;

/// Default maximum length of the instance and function names in a [Header]
const DEFAULT_MAX_NAME_LEN: u32 = 1 << 10;

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
fn error_data_after_end() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "received data after value end",
    )
}

/// Returns the error for a connection, on which receiving an invocation header failed
fn error_connection_failed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "connection failed")
}

/// Returns the error for data received for a closed receiver
fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed")
}
