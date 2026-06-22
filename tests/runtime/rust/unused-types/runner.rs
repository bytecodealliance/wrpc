pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    foo::bar::component::ping(clt, ()).await?;
    Ok(())
}
