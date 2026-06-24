use crate::client::exports::bar;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    bar(wrpc, ()).await?;
    Ok(())
}
