use crate::client::exports::bar;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    bar(tx, rx).await?;
    Ok(())
}
