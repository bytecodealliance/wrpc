//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::my::test::i::return_string;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let got = return_string(tx, rx).await?;
    assert_eq!(got, "hello");
    Ok(())
}
