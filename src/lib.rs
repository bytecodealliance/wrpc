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

#[doc(hidden)]
pub use wit_bindgen_wrpc as __wit_bindgen_wrpc;

/// Generates bindings for a WIT world, see [`wit_bindgen_wrpc::generate!`] for the input forms
/// and options.
///
/// It sets `runtime_path`, and with it every `*_path` default, to this crate, referred to as
/// `::wrpc`.
#[macro_export]
macro_rules! generate {
    () => {
        $crate::generate!({});
    };
    ($world:literal) => {
        $crate::generate!({ world: $world });
    };
    (in $path:literal) => {
        $crate::generate!({ path: $path });
    };
    ($world:literal in $path:literal) => {
        $crate::generate!({ world: $world, path: $path });
    };
    ({ $($opts:tt)* }) => {
        $crate::__wit_bindgen_wrpc::generate!({
            runtime_path: "::wrpc::__wit_bindgen_wrpc",
            $($opts)*
        });
    };
}
