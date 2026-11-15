use crate::client::test::xcrate::b_exports::{X, b};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    b(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    let x = X::new(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    X::foo(tx, rx, &x.as_borrow()).await?;
    Ok(())
}
