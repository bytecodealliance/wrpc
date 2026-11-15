pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::a::b::x::f1(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::other::c::x::f2(tx, rx).await?;
    Ok(())
}
