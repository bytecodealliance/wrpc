//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    one_argument(&mut tx, &mut rx, 1).await?;
    let v = one_result(&mut tx, &mut rx).await?;
    assert_eq!(v, 2);
    let v = one_argument_and_result(&mut tx, &mut rx, 3).await?;
    assert_eq!(v, 4);
    two_arguments(&mut tx, &mut rx, 5, 6).await?;
    let v = two_arguments_and_result(&mut tx, &mut rx, 7, 8).await?;
    assert_eq!(v, 9);
    Ok(())
}
