use crate::client::exports::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let res = test_imports(&mut tx, &mut rx).await?;
    res.unwrap();

    let x = X::new(&mut tx, &mut rx, 5).await?;
    let v = X::get_a(&mut tx, &mut rx, &x.as_borrow()).await?;
    assert_eq!(v, 5);
    X::set_a(&mut tx, &mut rx, &x.as_borrow(), 10).await?;
    let v = X::get_a(&mut tx, &mut rx, &x.as_borrow()).await?;
    assert_eq!(v, 10);
    let z1 = Z::new(&mut tx, &mut rx, 10).await?;
    let v = Z::get_a(&mut tx, &mut rx, &z1.as_borrow()).await?;
    assert_eq!(v, 10);
    let z2 = Z::new(&mut tx, &mut rx, 20).await?;
    let v = Z::get_a(&mut tx, &mut rx, &z2.as_borrow()).await?;
    assert_eq!(v, 20);

    let xadd = X::add(&mut tx, &mut rx, &x, 5).await?;
    let v = X::get_a(&mut tx, &mut rx, &xadd.as_borrow()).await?;
    assert_eq!(v, 15);

    let zadd = add(&mut tx, &mut rx, &z1.as_borrow(), &z2.as_borrow()).await?;
    let v = Z::get_a(&mut tx, &mut rx, &zadd.as_borrow()).await?;
    assert_eq!(v, 30);

    Z::num_dropped(&mut tx, &mut rx).await?;
    drop(z1);
    drop(z2);
    consume(&mut tx, &mut rx, &xadd).await?;
    Ok(())
}
