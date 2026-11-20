pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    use crate::client::test::dep0_1_0::test as v1;
    let mut clt = wrpc.connect().await?;
    assert_eq!(v1::x(&mut clt).await?, 1.0);
    assert_eq!(v1::y(&mut clt, 1.0).await?, 2.0);

    use crate::client::test::dep0_2_0::test as v2;
    assert_eq!(v2::x(&mut clt).await?, 2.0);
    assert_eq!(v2::z(&mut clt, 1.0, 1.0).await?, 4.0);
    Ok(())
}
