pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::a::b::the_test::x(clt).await?;
    Ok(())
}
