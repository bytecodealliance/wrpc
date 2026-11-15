//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    one_argument(tx, rx, 1).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = one_result(tx, rx).await?;
    assert_eq!(v, 2);
    let (tx, rx) = wrpc.connect().await?;
    let v = one_argument_and_result(tx, rx, 3).await?;
    assert_eq!(v, 4);
    let (tx, rx) = wrpc.connect().await?;
    two_arguments(tx, rx, 5, 6).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = two_arguments_and_result(tx, rx, 7, 8).await?;
    assert_eq!(v, 9);
    Ok(())
}
