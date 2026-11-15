use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    foo(tx, rx, "hello").await?;

    let (tx, rx) = wrpc.connect().await?;
    let t: String = bar(tx, rx).await?;
    assert_eq!(t, "world");
    Ok(())
}
