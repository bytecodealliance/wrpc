//@ args = '--generate-all'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::foo::baz::a::x(wrpc, ()).await?;
    Ok(())
}
