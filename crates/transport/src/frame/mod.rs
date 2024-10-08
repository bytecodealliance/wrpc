use std::sync::Arc;

use bytes::Bytes;

mod codec;
mod conn;

#[cfg(feature = "net")]
pub mod tcp;
#[cfg(all(unix, feature = "net"))]
pub mod unix;

pub use codec::*;
pub use conn::*;

/// Framing protocol version
pub const PROTOCOL: u8 = 0;

/// Owned wRPC frame
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub path: Arc<[usize]>,
    pub data: Bytes,
}

/// wRPC frame reference
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameRef<'a> {
    pub path: &'a [usize],
    pub data: &'a [u8],
}

impl<'a> From<&'a Frame> for FrameRef<'a> {
    fn from(Frame { path, data }: &'a Frame) -> Self {
        Self {
            path: path.as_ref(),
            data: data.as_ref(),
        }
    }
}
