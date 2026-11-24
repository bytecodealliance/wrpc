//@ wasmtime-flags = '-Wcomponent-model-async'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::test::common::i_runner::f(clt).await?;
    Ok(())
}
