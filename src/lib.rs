pub mod transport {
    pub use wrpc_transport::*;

    #[cfg(feature = "nats")]
    pub use wrpc_transport_nats as nats;

    #[cfg(feature = "quic")]
    pub use wrpc_transport_quic as quic;
}

pub mod runtime {
    #[cfg(feature = "wasmtime")]
    pub use wrpc_runtime_wasmtime as wasmtime;
}

pub use wit_bindgen_wrpc::generate;

pub use transport::{Index, Invoke, Serve};
