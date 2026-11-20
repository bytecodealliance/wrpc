use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    foo(&mut clt, &Bytes::from_static(b"hello")).await?;

    let t: Bytes = bar(&mut clt).await?;
    assert_eq!(t, &b"world"[..]);
    Ok(())
}
