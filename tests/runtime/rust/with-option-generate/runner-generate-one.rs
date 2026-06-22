//@ args = '--with=foo:baz/a=generate'

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    foo::baz::a::x(clt, ()).await?;
    Ok(())
}
