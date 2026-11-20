use crate::client::exports::bar;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    bar(clt).await?;
    Ok(())
}
