//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    one_argument(wrpc, (), 1).await?;
    let v = one_result(wrpc, ()).await?;
    assert_eq!(v, 2);
    let v = one_argument_and_result(wrpc, (), 3).await?;
    assert_eq!(v, 4);
    two_arguments(wrpc, (), 5, 6).await?;
    let v = two_arguments_and_result(wrpc, (), 7, 8).await?;
    assert_eq!(v, 9);
    Ok(())
}
