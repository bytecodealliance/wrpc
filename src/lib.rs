//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]

pub use wrpc_core::*;

#[cfg(feature = "http")]
pub use wrpc_http as http;

#[cfg(feature = "wasmtime")]
pub use wrpc_wasmtime as wasmtime;

#[cfg(feature = "wave")]
pub use wrpc_wave as wave;
