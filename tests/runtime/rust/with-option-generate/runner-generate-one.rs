//@ args = '--with foo:baz/a=generate'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::foo::baz::a::x(clt).await?;
    Ok(())
}
