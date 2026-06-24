pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::my::inline::foo1::foo(wrpc, ()).await?;
    crate::client::my::inline::foo2::foo(wrpc, ()).await?;
    crate::client::my::inline::bar1::bar(wrpc, ()).await?;
    crate::client::my::inline::bar2::bar(wrpc, ()).await?;
    Ok(())
}
