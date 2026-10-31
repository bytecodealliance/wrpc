use crate::client::test::resource_borrow::to_test::{foo, Thing};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing = Thing::new(wrpc, (), 254).await?;
    let v = foo(wrpc, (), &thing.as_borrow()).await?;
    assert_eq!(v, 254 + 1 + 2);
    Ok(())
}
