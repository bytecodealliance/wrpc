use crate::client::exports::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let res = test_imports(tx, rx).await?;
    res.unwrap();

    let (tx, rx) = wrpc.connect().await?;
    let x = X::new(tx, rx, 5).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = X::get_a(tx, rx, &x.as_borrow()).await?;
    assert_eq!(v, 5);
    let (tx, rx) = wrpc.connect().await?;
    X::set_a(tx, rx, &x.as_borrow(), 10).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = X::get_a(tx, rx, &x.as_borrow()).await?;
    assert_eq!(v, 10);
    let (tx, rx) = wrpc.connect().await?;
    let z1 = Z::new(tx, rx, 10).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Z::get_a(tx, rx, &z1.as_borrow()).await?;
    assert_eq!(v, 10);
    let (tx, rx) = wrpc.connect().await?;
    let z2 = Z::new(tx, rx, 20).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Z::get_a(tx, rx, &z2.as_borrow()).await?;
    assert_eq!(v, 20);

    let (tx, rx) = wrpc.connect().await?;
    let xadd = X::add(tx, rx, &x, 5).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = X::get_a(tx, rx, &xadd.as_borrow()).await?;
    assert_eq!(v, 15);

    let (tx, rx) = wrpc.connect().await?;
    let zadd = add(tx, rx, &z1.as_borrow(), &z2.as_borrow()).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Z::get_a(tx, rx, &zadd.as_borrow()).await?;
    assert_eq!(v, 30);

    let (tx, rx) = wrpc.connect().await?;
    Z::num_dropped(tx, rx).await?;
    drop(z1);
    drop(z2);
    let (tx, rx) = wrpc.connect().await?;
    consume(tx, rx, &xadd).await?;
    Ok(())
}
