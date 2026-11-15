//@ args = '--raw-strings'

use crate::client::cat::{bar, foo};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    foo(tx, rx, b"hello").await?;

    let (tx, rx) = wrpc.connect().await?;
    let t: Vec<u8> = bar(tx, rx).await?;
    assert_eq!(t, b"world");
    Ok(())
}
