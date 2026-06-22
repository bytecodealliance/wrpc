pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    my::test::exports_iface::bar(clt, ()).await?;
    Ok(())
}
