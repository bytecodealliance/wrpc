pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    use crate::client::test::dep0_1_0::test as v1;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(v1::x(tx, rx).await?, 1.0);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(v1::y(tx, rx, 1.0).await?, 2.0);

    use crate::client::test::dep0_2_0::test as v2;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(v2::x(tx, rx).await?, 2.0);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(v2::z(tx, rx, 1.0, 1.0).await?, 4.0);
    Ok(())
}
