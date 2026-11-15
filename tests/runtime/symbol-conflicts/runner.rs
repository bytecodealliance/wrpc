pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::my::inline::foo1::foo(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::my::inline::foo2::foo(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::my::inline::bar1::bar(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::my::inline::bar2::bar(tx, rx).await?;
    Ok(())
}
