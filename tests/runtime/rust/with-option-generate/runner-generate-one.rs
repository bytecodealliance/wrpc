//@ args = '--with foo:baz/a=generate'

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::foo::baz::a::x(wrpc, ()).await?;
    Ok(())
}
