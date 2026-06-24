use crate::client::test::resource_alias::e1::{a as a1, Foo as Foo1, X};
use crate::client::test::resource_alias::e2::{a as a2, Foo as Foo2};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let x = X::new(wrpc, (), 42).await?;
    a1(wrpc, (), &Foo1 { x }).await?;

    let x = X::new(wrpc, (), 7).await?;
    let foo_e2 = Foo2 { x };
    let x = X::new(wrpc, (), 8).await?;
    let bar_e2 = Foo1 { x };
    let y = X::new(wrpc, (), 8).await?;
    a2(wrpc, (), &foo_e2, &bar_e2, &y.as_borrow()).await?;
    Ok(())
}
