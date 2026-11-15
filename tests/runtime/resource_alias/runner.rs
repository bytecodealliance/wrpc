use crate::client::test::resource_alias::e1::{a as a1, Foo as Foo1, X};
use crate::client::test::resource_alias::e2::{a as a2, Foo as Foo2};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let x = X::new(tx, rx, 42).await?;
    let (tx, rx) = wrpc.connect().await?;
    a1(tx, rx, &Foo1 { x }).await?;

    let (tx, rx) = wrpc.connect().await?;
    let x = X::new(tx, rx, 7).await?;
    let foo_e2 = Foo2 { x };
    let (tx, rx) = wrpc.connect().await?;
    let x = X::new(tx, rx, 8).await?;
    let bar_e2 = Foo1 { x };
    let (tx, rx) = wrpc.connect().await?;
    let y = X::new(tx, rx, 8).await?;
    let (tx, rx) = wrpc.connect().await?;
    a2(tx, rx, &foo_e2, &bar_e2, &y.as_borrow()).await?;
    Ok(())
}
