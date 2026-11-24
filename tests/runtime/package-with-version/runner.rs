pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    let _ = crate::client::my::inline::foo::Bar::new(clt).await?;
    Ok(())
}
