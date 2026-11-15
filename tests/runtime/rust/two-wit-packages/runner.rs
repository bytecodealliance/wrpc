pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    crate::client::a::b::x::f1(&mut tx, &mut rx).await?;
    crate::client::other::c::x::f2(&mut tx, &mut rx).await?;
    Ok(())
}
