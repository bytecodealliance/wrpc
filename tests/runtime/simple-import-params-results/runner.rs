//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    one_argument(&mut clt, 1).await?;
    let v = one_result(&mut clt).await?;
    assert_eq!(v, 2);
    let v = one_argument_and_result(&mut clt, 3).await?;
    assert_eq!(v, 4);
    two_arguments(&mut clt, 5, 6).await?;
    let v = two_arguments_and_result(&mut clt, 7, 8).await?;
    assert_eq!(v, 9);
    Ok(())
}
