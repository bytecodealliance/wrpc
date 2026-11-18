//@ args = '--features y'

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    crate::client::foo::bar::bindings::y(&mut tx, &mut rx).await?;
    crate::client::foo::bar::bindings::z(&mut tx, &mut rx).await?;
    Ok(())
}
