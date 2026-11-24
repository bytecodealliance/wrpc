use crate::client::exports::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let res = test_imports(&mut clt).await?;
    res.unwrap();

    let x = X::new(&mut clt, 5).await?;
    let v = X::get_a(&mut clt, &x.as_borrow()).await?;
    assert_eq!(v, 5);
    X::set_a(&mut clt, &x.as_borrow(), 10).await?;
    let v = X::get_a(&mut clt, &x.as_borrow()).await?;
    assert_eq!(v, 10);
    let z1 = Z::new(&mut clt, 10).await?;
    let v = Z::get_a(&mut clt, &z1.as_borrow()).await?;
    assert_eq!(v, 10);
    let z2 = Z::new(&mut clt, 20).await?;
    let v = Z::get_a(&mut clt, &z2.as_borrow()).await?;
    assert_eq!(v, 20);

    let xadd = X::add(&mut clt, &x, 5).await?;
    let v = X::get_a(&mut clt, &xadd.as_borrow()).await?;
    assert_eq!(v, 15);

    let zadd = add(&mut clt, &z1.as_borrow(), &z2.as_borrow()).await?;
    let v = Z::get_a(&mut clt, &zadd.as_borrow()).await?;
    assert_eq!(v, 30);

    Z::num_dropped(&mut clt).await?;
    drop(z1);
    drop(z2);
    consume(&mut clt, &xadd).await?;
    Ok(())
}
