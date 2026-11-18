use crate::client::test::xcrate::b_exports::{X, b};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    b(&mut tx, &mut rx).await?;
    let x = X::new(&mut tx, &mut rx).await?;
    X::foo(&mut tx, &mut rx, &x.as_borrow()).await?;
    Ok(())
}
