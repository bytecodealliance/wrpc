pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let _ = crate::client::my::inline::foo::Bar::new(tx, rx).await?;
    Ok(())
}
