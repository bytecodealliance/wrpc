use crate::client::exports::Float as Float2;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let a = Float::new(&mut tx, &mut rx, 42.0).await?;
    let b = Float::new(&mut tx, &mut rx, 55.0).await?;
    let float3 = add(&mut tx, &mut rx, &a.as_borrow(), &b.as_borrow()).await?;
    let v = Float::get(&mut tx, &mut rx, &float3.as_borrow()).await?;
    assert_eq!(v, 114.0);

    let float3 = Float2::new(&mut tx, &mut rx, 22.0).await?;
    let v = Float2::get(&mut tx, &mut rx, &float3.as_borrow()).await?;
    assert_eq!(v, 22. + 1. + 2. + 4. + 3.);

    let res = Float2::add(&mut tx, &mut rx, &float3, 7.0).await?;
    let v = Float2::get(&mut tx, &mut rx, &res.as_borrow()).await?;
    assert_eq!(v, 59.0);
    Ok(())
}
