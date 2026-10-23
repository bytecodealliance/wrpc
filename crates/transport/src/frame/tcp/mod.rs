//! wRPC TCP transport

#[cfg(feature = "net")]
pub mod tokio;
#[cfg(feature = "net")]
pub use tokio::*;

#[cfg(all(target_family = "wasm", feature = "wasi"))]
pub mod wasi;
#[cfg(all(target_family = "wasm", feature = "wasi", not(feature = "net")))]
pub use wasi::*;
