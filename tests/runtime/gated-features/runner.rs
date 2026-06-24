//@ args = '--features y'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::runner::foo::bar::bindings::y(wrpc, ()).await?;
    crate::runner::foo::bar::bindings::z(wrpc, ()).await?;
    Ok(())
}
