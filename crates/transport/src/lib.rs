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
//! invocation and transmitting the results, [`serve()`] uses the default codecs.
//!
//! A transport establishes bidirectional byte streams, each carrying invocations one after
//! another, which it yields as any [`AsyncWrite`](tokio::io::AsyncWrite)/
//! [`AsyncRead`](tokio::io::AsyncRead) byte stream pair. [`transmit()`] and [`receive()`] layer
//! the wRPC [framing](FrameDecoder) protocol on top of it to multiplex the asynchronous values nested in
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
    FrameDecoder, FrameEncoder, FrameHeaderDecoder, FrameHeaderEncoder, FrameHeaderRef, FrameRef,
};
pub use header::{Header, HeaderDecoder, HeaderEncoder, HeaderRef, PROTOCOL};
pub use invoke::{Invocation, invoke, invoke_blocking};
pub use listener::Listener;
pub use mux::{Receipt, Transmission, receive, transmit};
pub use pack::{pack, unpack};
/// Awaiting the futures returned by [`invoke()`], [`serve()`], [`receive()`] and the methods of
/// [`Codec`], or
/// futures holding a [`Transmission`](frame::Transmission) across an await, in a [`Send`] future may fail to compile with "one type is more general than the other"
/// or "implementation of ... is not general enough" due to
/// [rust-lang/rust#96865](https://github.com/rust-lang/rust/issues/96865),
/// call [`send`](SendFuture::send) on them before awaiting to work around it:
///
/// ```
/// use futures::StreamExt as _;
/// use futures::stream::BoxStream;
/// use tokio::io::{AsyncRead, AsyncWrite};
/// use tokio::try_join;
/// use tokio_util::codec::{FramedRead, FramedWrite};
/// use wrpc_transport::{FrameDecoder, FrameEncoder, SendFuture as _, invoke};
///
/// fn invoke_send(
///     tx: impl AsyncWrite + Send + Unpin,
///     rx: impl AsyncRead + Send + Unpin,
/// ) -> impl Future<Output = std::io::Result<Vec<String>>> + Send {
///     async {
///         let tx = FramedWrite::new(tx, FrameEncoder);
///         let rx = FramedRead::new(rx, FrameDecoder::default());
///         let ((s,), io) = invoke::<_, (BoxStream<'static, Vec<String>>,), _, _, _, _>(
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
pub use serve::serve;
pub use value::*;

use bytes::{Buf, BufMut as _, BytesMut};
use tokio_util::codec::Encoder as _;

/// Codec of the values of invocations
#[derive(Clone, Copy, Debug, Default)]
pub struct Codec<E, D> {
    /// Encoder of the values sent, the parameters when invoking and the results when serving
    pub enc: E,
    /// Decoder of the values received, the results when invoking and the parameters when serving
    pub dec: D,
}

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
