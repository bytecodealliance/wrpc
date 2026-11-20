pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    crate::client::my::inline::foo1::foo(&mut clt).await?;
    crate::client::my::inline::foo2::foo(&mut clt).await?;
    crate::client::my::inline::bar1::bar(&mut clt).await?;
    crate::client::my::inline::bar2::bar(&mut clt).await?;
    Ok(())
}
