#![allow(clippy::type_complexity)]
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
//! Implementations of [Invoke] and [Serve] define transport-specific, multiplexed bidirectional byte stream types:
//! - [`Invoke::Incoming`] and [`Serve::Incoming`] represent the stream *incoming* from a peer.
//! - [`Invoke::Outgoing`] and [`Serve::Outgoing`] represent the stream *outgoing* to a peer.

pub mod frame;
pub mod invoke;
pub mod serve;

mod value;

pub use frame::{
    Accept, Decoder as FrameDecoder, Encoder as FrameEncoder, Frame, FrameRef, Server,
};
pub use invoke::{Invoke, InvokeExt};
pub use send_future::SendFuture;
pub use serve::{Serve, ServeExt};
pub use value::*;

#[cfg(any(target_family = "wasm", feature = "net"))]
pub use frame::tcp;
#[cfg(all(unix, feature = "net"))]
pub use frame::unix;

use core::mem;
use core::pin::{pin, Pin};
use core::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};
use tracing::trace;

/// Internal workaround trait
///
/// This is an internal trait used as a workaround for
/// https://github.com/rust-lang/rust/issues/63033
#[doc(hidden)]
pub trait Captures<'a> {}

impl<'a, T: ?Sized> Captures<'a> for T {}

/// Multiplexes streams
///
/// Implementations of this trait define multiplexing for underlying connections
/// using a particular structural `path`
pub trait Index<T> {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> anyhow::Result<T>;
}

/// Buffered incoming stream used for decoding values
pub struct Incoming<T> {
    buffer: BytesMut,
    inner: T,
}

impl<T: Index<T>> Index<Self> for Incoming<T> {
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        let inner = self.inner.index(path)?;
        Ok(Self {
            buffer: BytesMut::default(),
            inner,
        })
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Incoming<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let cap = buf.remaining();
        if cap == 0 {
            trace!("attempt to read empty buffer");
            return Poll::Ready(Ok(()));
        }
        if !self.buffer.is_empty() {
            if self.buffer.len() > cap {
                trace!(cap, len = self.buffer.len(), "reading part of buffer");
                buf.put_slice(&self.buffer.split_to(cap));
            } else {
                trace!(cap, len = self.buffer.len(), "reading full buffer");
                buf.put_slice(&mem::take(&mut self.buffer));
            }
            return Poll::Ready(Ok(()));
        }
        pin!(&mut self.inner).poll_read(cx, buf)
    }
}
