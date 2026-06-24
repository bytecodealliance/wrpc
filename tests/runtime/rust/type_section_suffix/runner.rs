pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::test::suffix::imports::foo(wrpc, ()).await?;
    crate::client::foo::f(wrpc, ()).await?;
    crate::client::bar::f(wrpc, ()).await?;
    Ok(())
}
