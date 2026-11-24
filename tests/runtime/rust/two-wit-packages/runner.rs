pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    crate::client::a::b::x::f1(&mut clt).await?;
    crate::client::other::c::x::f2(&mut clt).await?;
    Ok(())
}
