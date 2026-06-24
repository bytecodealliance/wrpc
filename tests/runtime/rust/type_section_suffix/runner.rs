pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::runner::test::suffix::imports::foo(wrpc, ()).await?;
    crate::runner::foo::f(wrpc, ()).await?;
    crate::runner::bar::f(wrpc, ()).await?;
    Ok(())
}
