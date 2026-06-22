pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    let flag = my::inline::flags_iface::get_flag(clt, ()).await?;
    assert_eq!(flag, my::inline::flags_iface::Bar::BAZ);
    Ok(())
}
