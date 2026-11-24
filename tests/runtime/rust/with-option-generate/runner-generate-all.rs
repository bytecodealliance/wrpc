//@ args = '--generate-all'

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::foo::baz::a::x(clt).await?;
    Ok(())
}
