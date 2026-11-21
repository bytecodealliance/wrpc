//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]

pub use wrpc_transport::*;

#[cfg(feature = "http")]
pub use wrpc_http as http;

#[cfg(feature = "wasmtime")]
pub use wrpc_wasmtime as wasmtime;
