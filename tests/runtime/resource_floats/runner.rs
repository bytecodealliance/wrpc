use crate::client::exports::Float as Float2;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let a = Float::new(tx, rx, 42.0).await?;
    let (tx, rx) = wrpc.connect().await?;
    let b = Float::new(tx, rx, 55.0).await?;
    let (tx, rx) = wrpc.connect().await?;
    let float3 = add(tx, rx, &a.as_borrow(), &b.as_borrow()).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Float::get(tx, rx, &float3.as_borrow()).await?;
    assert_eq!(v, 114.0);

    let (tx, rx) = wrpc.connect().await?;
    let float3 = Float2::new(tx, rx, 22.0).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Float2::get(tx, rx, &float3.as_borrow()).await?;
    assert_eq!(v, 22. + 1. + 2. + 4. + 3.);

    let (tx, rx) = wrpc.connect().await?;
    let res = Float2::add(tx, rx, &float3, 7.0).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Float2::get(tx, rx, &res.as_borrow()).await?;
    assert_eq!(v, 59.0);
    Ok(())
}
