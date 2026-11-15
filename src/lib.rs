//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]

/// wRPC transport
pub mod transport {
    pub use wrpc_transport::*;

    #[cfg(feature = "http")]
    pub use wrpc_http as http;

    #[cfg(feature = "websockets")]
    pub use wrpc_websockets as websockets;
}

/// wRPC runtime
pub mod runtime {
    #[cfg(feature = "wasmtime")]
    pub use wrpc_wasmtime as wasmtime;
}
