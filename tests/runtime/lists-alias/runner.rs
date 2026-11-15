use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    foo(tx, rx, &Bytes::from_static(b"hello")).await?;

    let (tx, rx) = wrpc.connect().await?;
    let t: Bytes = bar(tx, rx).await?;
    assert_eq!(t, &b"world"[..]);
    Ok(())
}
