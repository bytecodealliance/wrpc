use crate::client::exports::Float as Float2;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let a = Float::new(wrpc, (), 42.0).await?;
    let b = Float::new(wrpc, (), 55.0).await?;
    let float3 = add(wrpc, (), &a.as_borrow(), &b.as_borrow()).await?;
    let v = Float::get(wrpc, (), &float3.as_borrow()).await?;
    assert_eq!(v, 114.0);

    let float3 = Float2::new(wrpc, (), 22.0).await?;
    let v = Float2::get(wrpc, (), &float3.as_borrow()).await?;
    assert_eq!(v, 22. + 1. + 2. + 4. + 3.);

    let res = Float2::add(wrpc, (), &float3, 7.0).await?;
    let v = Float2::get(wrpc, (), &res.as_borrow()).await?;
    assert_eq!(v, 59.0);
    Ok(())
}
