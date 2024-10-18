//! wRPC TCP transport

#[cfg(feature = "net")]
pub mod tokio;
#[cfg(feature = "net")]
pub use tokio::*;

#[cfg(target_family = "wasm")]
pub mod wasi;
#[cfg(all(target_family = "wasm", not(feature = "net")))]
pub use wasi::*;
