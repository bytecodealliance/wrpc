//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use wrpc_core::*;

#[cfg(feature = "http")]
#[doc(inline)]
pub use wrpc_http as http;

#[cfg(feature = "wasmtime")]
#[doc(inline)]
pub use wrpc_wasmtime as wasmtime;

#[cfg(feature = "wave")]
#[doc(inline)]
pub use wrpc_wave as wave;
