//@ wasmtime-flags = '-Wcomponent-model-async'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::test::common::i_runner::f(wrpc, ()).await
}
