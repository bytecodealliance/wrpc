use crate::client::cat::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    foo(&mut clt, "hello").await?;

    let t: String = bar(&mut clt).await?;
    assert_eq!(t, "world");
    Ok(())
}
