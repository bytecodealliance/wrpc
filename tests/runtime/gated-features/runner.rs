//@ args = '--features y'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::foo::bar::bindings::y(wrpc, ()).await?;
    crate::client::foo::bar::bindings::z(wrpc, ()).await?;
    Ok(())
}
