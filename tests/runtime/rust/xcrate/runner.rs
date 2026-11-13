use crate::client::test::xcrate::b_exports::{X, b};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    b(wrpc, ()).await?;
    let x = X::new(wrpc, ()).await?;
    X::foo(wrpc, (), &x.as_borrow()).await?;
    Ok(())
}
