pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    crate::client::a::b::x::f1(&mut clt).await?;
    crate::client::other::c::x::f2(&mut clt).await?;
    Ok(())
}
