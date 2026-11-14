//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::my::test::i::return_string;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let got = return_string(wrpc, ()).await?;
    assert_eq!(got, "hello");
    Ok(())
}
