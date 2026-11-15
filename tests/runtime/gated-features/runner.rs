//@ args = '--features y'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::foo::bar::bindings::y(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::foo::bar::bindings::z(tx, rx).await?;
    Ok(())
}
