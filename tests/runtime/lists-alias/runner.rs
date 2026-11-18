use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    foo(&mut tx, &mut rx, &Bytes::from_static(b"hello")).await?;

    let t: Bytes = bar(&mut tx, &mut rx).await?;
    assert_eq!(t, &b"world"[..]);
    Ok(())
}
