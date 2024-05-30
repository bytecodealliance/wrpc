pub mod transport {
    pub use wrpc_transport::*;
    #[cfg(feature = "nats")]
    pub use wrpc_transport_nats as nats;
}

pub mod runtime {
    #[cfg(feature = "wasmtime")]
    pub use wrpc_runtime_wasmtime as wasmtime;
}

pub mod interfaces {
    pub use wrpc_interface_blobstore as blobstore;
    pub use wrpc_interface_http as http;
}

pub use wit_bindgen_wrpc::generate;

pub use transport::{Index, Invoke, Serve};
