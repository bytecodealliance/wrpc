//@ args = '--features y'

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    foo::bar::iface::y(clt, ()).await?;
    foo::bar::iface::z(clt, ()).await?;
    Ok(())
}
