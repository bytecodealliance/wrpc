use crate::client::cat::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    foo(wrpc, (), "hello").await?;

    let t: String = bar(wrpc, ()).await?;
    assert_eq!(t, "world");
    Ok(())
}
