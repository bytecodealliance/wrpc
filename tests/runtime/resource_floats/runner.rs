use crate::client::exports::Float as Float2;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let a = Float::new(&mut clt, 42.0).await?;
    let b = Float::new(&mut clt, 55.0).await?;
    let float3 = add(&mut clt, &a.as_borrow(), &b.as_borrow()).await?;
    let v = Float::get(&mut clt, &float3.as_borrow()).await?;
    assert_eq!(v, 114.0);

    let float3 = Float2::new(&mut clt, 22.0).await?;
    let v = Float2::get(&mut clt, &float3.as_borrow()).await?;
    assert_eq!(v, 22. + 1. + 2. + 4. + 3.);

    let res = Float2::add(&mut clt, &float3, 7.0).await?;
    let v = Float2::get(&mut clt, &res.as_borrow()).await?;
    assert_eq!(v, 59.0);
    Ok(())
}
