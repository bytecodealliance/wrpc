use crate::client::exports::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let res = test_imports(wrpc, ()).await?;
    res.unwrap();

    let x = X::new(wrpc, (), 5).await?;
    let v = X::get_a(wrpc, (), &x.as_borrow()).await?;
    assert_eq!(v, 5);
    X::set_a(wrpc, (), &x.as_borrow(), 10).await?;
    let v = X::get_a(wrpc, (), &x.as_borrow()).await?;
    assert_eq!(v, 10);
    let z1 = Z::new(wrpc, (), 10).await?;
    let v = Z::get_a(wrpc, (), &z1.as_borrow()).await?;
    assert_eq!(v, 10);
    let z2 = Z::new(wrpc, (), 20).await?;
    let v = Z::get_a(wrpc, (), &z2.as_borrow()).await?;
    assert_eq!(v, 20);

    let xadd = X::add(wrpc, (), &x, 5).await?;
    let v = X::get_a(wrpc, (), &xadd.as_borrow()).await?;
    assert_eq!(v, 15);

    let zadd = add(wrpc, (), &z1.as_borrow(), &z2.as_borrow()).await?;
    let v = Z::get_a(wrpc, (), &zadd.as_borrow()).await?;
    assert_eq!(v, 30);

    Z::num_dropped(wrpc, ()).await?;
    drop(z1);
    drop(z2);
    consume(wrpc, (), &xadd).await?;
    Ok(())
}
