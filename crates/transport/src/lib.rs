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
