//@ wasmtime-flags = '-Wcomponent-model-async'

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::a::b::i::f(clt).await?;
    Ok(())
}
