use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    foo(&mut tx, &mut rx, "hello").await?;

    let t: String = bar(&mut tx, &mut rx).await?;
    assert_eq!(t, "world");
    Ok(())
}
