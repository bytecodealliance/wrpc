use crate::client::test::resource_alias::e1::{a as a1, Foo as Foo1, X};
use crate::client::test::resource_alias::e2::{a as a2, Foo as Foo2};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let x = X::new(&mut tx, &mut rx, 42).await?;
    a1(&mut tx, &mut rx, &Foo1 { x }).await?;

    let x = X::new(&mut tx, &mut rx, 7).await?;
    let foo_e2 = Foo2 { x };
    let x = X::new(&mut tx, &mut rx, 8).await?;
    let bar_e2 = Foo1 { x };
    let y = X::new(&mut tx, &mut rx, 8).await?;
    a2(&mut tx, &mut rx, &foo_e2, &bar_e2, &y.as_borrow()).await?;
    Ok(())
}
