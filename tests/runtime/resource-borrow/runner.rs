use crate::client::test::resource_borrow::to_test::{foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let thing = Thing::new(&mut tx, &mut rx, 254).await?;
    let v = foo(&mut tx, &mut rx, &thing.as_borrow()).await?;
    assert_eq!(v, 254 + 1 + 2);
    Ok(())
}
