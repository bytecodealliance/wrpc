//! wRPC is an RPC framework based on WIT

#![deny(missing_docs)]

/// wRPC transport
pub mod transport {
    pub use wrpc_transport::*;

    #[cfg(feature = "nats")]
    pub use wrpc_transport_nats as nats;

    #[cfg(feature = "quic")]
    pub use wrpc_transport_quic as quic;

    #[cfg(feature = "web-transport")]
    pub use wrpc_transport_web as web;
}

/// wRPC runtime
pub mod runtime {
    #[cfg(feature = "wasmtime")]
    pub use wrpc_runtime_wasmtime as wasmtime;
}

pub use transport::{Index, Invoke, Serve};
