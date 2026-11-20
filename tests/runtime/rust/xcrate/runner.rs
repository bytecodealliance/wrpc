use crate::client::test::xcrate::b_exports::{X, b};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    b(&mut clt).await?;
    let x = X::new(&mut clt).await?;
    X::foo(&mut clt, &x.as_borrow()).await?;
    Ok(())
}
