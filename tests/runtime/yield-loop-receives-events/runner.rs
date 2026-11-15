//@ wasmtime-flags = '-Wcomponent-model-async'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::test::common::i_runner::f(tx, rx).await
}
