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
//! - [`Invoker`] - the client side, which *invokes* WIT functions over the byte streams of an
//!   invocation, [`invoke()`] uses the default codecs
//! - [`Server`] - the server side, which *serves* WIT functions over the frames following the
//!   header read by [`HeaderDecoder`] from the byte streams of an invocation, for example ones
//!   accepted by a transport, [`serve()`] uses the default codecs
//!
//! A transport establishes bidirectional byte streams, each carrying invocations one after
//! another, which it yields as any [`AsyncWrite`](tokio::io::AsyncWrite)/
//! [`AsyncRead`](tokio::io::AsyncRead) byte stream pair. [`Invoker`] and [`Server`] layer the
//! wRPC [framing](frame) protocol on top of it to multiplex the asynchronous values nested in
//! parameters and results, which encoders [Transmit] and decoders [Receive].
//!
//! Over TCP, enable `TCP_NODELAY`, since invocations consist of small writes, each awaited by
//! the peer.

pub mod frame;
#[cfg(all(target_family = "wasm", feature = "wasi"))]
pub mod tcp;

mod header;
mod invoke;
mod mux;
mod pack;
mod serve;
mod value;

pub use frame::{
    Decoder as FrameDecoder, Encoder as FrameEncoder, FrameHeaderDecoder, FrameHeaderEncoder,
    FrameHeaderRef, FrameRef,
};
pub use header::{Header, HeaderDecoder, HeaderEncoder, HeaderReadError, HeaderRef, PROTOCOL};
pub use invoke::{Invocation, Invoker, Pending, invoke, invoke_blocking};
pub use pack::{pack, unpack};
/// Awaiting the futures returned by [`invoke()`], [`serve()`] and the methods of [`Invoker`],
/// [`Pending`] and [`Server`], or futures holding an [`Outgoing`](frame::Outgoing) across an
/// await, in a [`Send`] future may fail to compile with "one type is more general than the other"
/// or "implementation of ... is not general enough" due to
/// [rust-lang/rust#96865](https://github.com/rust-lang/rust/issues/96865),
/// call [`send`](SendFuture::send) on them before awaiting to work around it:
///
/// ```
/// use futures::StreamExt as _;
/// use futures::stream::BoxStream;
/// use tokio::io::{AsyncRead, AsyncWrite};
/// use tokio::try_join;
/// use wrpc_transport::{SendFuture as _, invoke};
///
/// fn invoke_send(
///     tx: impl AsyncWrite + Send + Unpin,
///     rx: impl AsyncRead + Send + Unpin,
/// ) -> impl Future<Output = std::io::Result<Vec<String>>> + Send {
///     async {
///         let ((s,), io) = invoke::<_, (BoxStream<'static, Vec<String>>,), _, _>(
///             tx,
///             rx,
///             "wrpc-test:integration/async",
///             "with-streams",
///             (),
///         )
///         .send()
///         .await?;
///         let (s, ()) = try_join!(async { Ok(s.concat().await) }, io)?;
///         Ok(s)
///     }
/// }
/// ```
pub use send_future::SendFuture;
pub use serve::{Server, serve};
pub use value::*;

use bytes::BytesMut;
use tokio_util::codec::Encoder as _;

/// Default maximum length of the instance and function names in a [Header]
const DEFAULT_MAX_NAME_LEN: u32 = 1 << 10;

/// Maximum length of the data of a frame sent
const MAX_FRAME_DATA: usize = 1 << 16;

/// Encodes `data` as frames on `path` into `dst`, encoding nothing if `data` is empty
fn encode_frames(path: &[usize], data: &[u8], dst: &mut BytesMut) -> std::io::Result<()> {
    for data in data.chunks(MAX_FRAME_DATA) {
        frame::Encoder.encode(FrameRef { path, data }, dst)?;
    }
    Ok(())
}

/// Encodes the frame closing `path` into `dst`
fn encode_close(path: &[usize], dst: &mut BytesMut) -> std::io::Result<()> {
    frame::Encoder.encode(FrameRef { path, data: &[] }, dst)
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

/// Returns the error for data received for a closed receiver
fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed")
}
