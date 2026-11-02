//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]

/// wRPC transport
pub mod transport {
    pub use wrpc_transport::*;

    #[cfg(feature = "http")]
    pub use wrpc_http as http;

    #[cfg(feature = "quic")]
    pub use wrpc_quic as quic;

    #[cfg(feature = "webtransport")]
    pub use wrpc_webtransport as web;
}

/// wRPC runtime
pub mod runtime {
    #[cfg(feature = "wasmtime")]
    pub use wrpc_wasmtime as wasmtime;
}

pub use transport::{Invoke, Serve};
