//@ args = '--features y'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    crate::client::foo::bar::bindings::y(&mut clt).await?;
    crate::client::foo::bar::bindings::z(&mut clt).await?;
    Ok(())
}
