//! wRPC TCP transport

#[cfg(feature = "net")]
pub mod tokio;
#[cfg(feature = "net")]
pub use tokio::*;

#[cfg(all(target_family = "wasm", feature = "wasi-net"))]
pub mod wasi;
#[cfg(all(target_family = "wasm", feature = "wasi-net", not(feature = "net")))]
pub use wasi::*;
