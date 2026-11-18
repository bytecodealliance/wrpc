pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    use crate::client::test::dep0_1_0::test as v1;
    let (mut tx, mut rx) = wrpc.connect().await?;
    assert_eq!(v1::x(&mut tx, &mut rx).await?, 1.0);
    assert_eq!(v1::y(&mut tx, &mut rx, 1.0).await?, 2.0);

    use crate::client::test::dep0_2_0::test as v2;
    assert_eq!(v2::x(&mut tx, &mut rx).await?, 2.0);
    assert_eq!(v2::z(&mut tx, &mut rx, 1.0, 1.0).await?, 4.0);
    Ok(())
}
