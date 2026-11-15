pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    crate::client::my::inline::foo1::foo(&mut tx, &mut rx).await?;
    crate::client::my::inline::foo2::foo(&mut tx, &mut rx).await?;
    crate::client::my::inline::bar1::bar(&mut tx, &mut rx).await?;
    crate::client::my::inline::bar2::bar(&mut tx, &mut rx).await?;
    Ok(())
}
