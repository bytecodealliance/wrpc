use crate::client::test::resource_borrow::to_test::{foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let thing = Thing::new(&mut clt, 254).await?;
    let v = foo(&mut clt, &thing.as_borrow()).await?;
    assert_eq!(v, 254 + 1 + 2);
    Ok(())
}
