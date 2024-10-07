use std::sync::Arc;

use bytes::Bytes;

mod codec;
mod conn;
pub mod tcp;
#[cfg(unix)]
pub mod unix;

pub use codec::*;
pub use conn::*;

pub const PROTOCOL: u8 = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub path: Arc<[usize]>,
    pub data: Bytes,
}

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
