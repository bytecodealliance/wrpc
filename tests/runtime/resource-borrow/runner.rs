use crate::client::test::resource_borrow::to_test::{foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let thing = Thing::new(tx, rx, 254).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = foo(tx, rx, &thing.as_borrow()).await?;
    assert_eq!(v, 254 + 1 + 2);
    Ok(())
}
