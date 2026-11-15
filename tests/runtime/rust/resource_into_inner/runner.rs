use crate::client::test::resource_into_inner::to_test::test;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    test(tx, rx).await?;
    Ok(())
}
