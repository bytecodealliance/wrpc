pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::a::b::x::f1(wrpc, ()).await?;
    crate::client::other::c::x::f2(wrpc, ()).await?;
    Ok(())
}
