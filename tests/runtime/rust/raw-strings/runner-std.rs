//@ args = '--raw-strings'

use crate::client::cat::{bar, foo};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    foo(&mut tx, &mut rx, b"hello").await?;

    let t: Vec<u8> = bar(&mut tx, &mut rx).await?;
    assert_eq!(t, b"world");
    Ok(())
}
