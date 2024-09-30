#![allow(clippy::type_complexity)]

#[cfg(feature = "frame")]
pub mod frame;
pub mod invoke;
pub mod serve;

mod value;

#[cfg(feature = "frame")]
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, FrameRef};
pub use invoke::{Invoke, InvokeExt};
pub use send_future::SendFuture;
pub use serve::{Serve, ServeExt};
pub use value::*;

use core::mem;
use core::pin::{pin, Pin};
use core::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};
use tracing::trace;

#[doc(hidden)]
// This is an internal trait used as a workaround for
// https://github.com/rust-lang/rust/issues/63033
pub trait Captures<'a> {}

impl<'a, T: ?Sized> Captures<'a> for T {}

/// `Index` implementations are capable of multiplexing underlying connections using a particular
/// structural `path`
pub trait Index<T> {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> anyhow::Result<T>;
}

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
