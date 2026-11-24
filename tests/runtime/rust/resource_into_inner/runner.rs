use crate::client::test::resource_into_inner::to_test::test;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    test(clt).await?;
    Ok(())
}
