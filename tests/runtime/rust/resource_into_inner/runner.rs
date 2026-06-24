use crate::runner::test::resource_into_inner::to_test::test;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    test(wrpc, ()).await?;
    Ok(())
}
