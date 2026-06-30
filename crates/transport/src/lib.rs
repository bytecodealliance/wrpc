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
//! [Invoke] and [Serve] establish a single bidirectional byte stream per invocation, over which
//! wRPC layers its [framing](frame) protocol to multiplex the nested async parameter and result
//! sub-streams. Both therefore yield the framed [`frame::Outgoing`] and [`frame::Incoming`]
//! streams regardless of the underlying transport, which only needs to provide any
//! [`AsyncWrite`](tokio::io::AsyncWrite)/[`AsyncRead`](tokio::io::AsyncRead) byte stream.

pub mod frame;
pub mod invoke;
pub mod serve;

mod value;

pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, Frame, FrameRef, Server};
pub use invoke::{Invoke, InvokeExt};
pub use serve::{Serve, ServeExt};
pub use value::*;

#[cfg(any(target_family = "wasm", feature = "net"))]
pub use frame::tcp;
#[cfg(all(unix, feature = "net"))]
pub use frame::unix;

use core::mem;
use core::pin::{Pin, pin};
use core::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};
use tracing::trace;

/// Buffered incoming stream used for decoding values
///
/// This wraps the multiplexed framed [`frame::Incoming`] stream with a read buffer used to
/// hold bytes that were read ahead while decoding synchronous values.
pub struct BufferedIncoming {
    buffer: BytesMut,
    inner: frame::Incoming,
}

impl BufferedIncoming {
    /// Index the incoming stream using a structural `path`, returning a handle to the
    /// multiplexed sub-stream addressed by it.
    pub fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        let inner = self.inner.index(path)?;
        Ok(Self {
            buffer: BytesMut::default(),
            inner,
        })
    }
}

impl AsyncRead for BufferedIncoming {
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
