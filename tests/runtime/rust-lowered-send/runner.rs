//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

// Explicitly require Send.
fn require_send<T: Send>(t: T) -> T {
    t
}

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    require_send(async { one_argument(wrpc, (), "hello").await }).await
}
