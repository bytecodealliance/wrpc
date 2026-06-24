use crate::client::test::resource_borrow::to_test::{foo, Thing};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing = Thing::new(wrpc, (), 42).await?;
    assert_eq!(foo(wrpc, (), &thing.as_borrow()).await?, 42 + 1 + 2);
    Ok(())
}
