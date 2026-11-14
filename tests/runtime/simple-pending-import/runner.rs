//@ wasmtime-flags = '-Wcomponent-model-async'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::a::b::i::f(wrpc, ()).await
}
