//@ args = '--raw-strings'

use crate::client::cat::{bar, foo};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    foo(&mut clt, b"hello").await?;

    let t: Vec<u8> = bar(&mut clt).await?;
    assert_eq!(t, b"world");
    Ok(())
}
