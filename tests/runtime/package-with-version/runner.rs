pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let _ = crate::runner::my::inline::foo::Bar::new(wrpc, ()).await?;
    Ok(())
}
