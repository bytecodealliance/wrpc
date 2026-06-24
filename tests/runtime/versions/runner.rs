pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    use crate::client::test::dep0_1_0::test as v1;
    assert_eq!(v1::x(wrpc, ()).await?, 1.0);
    assert_eq!(v1::y(wrpc, (), 1.0).await?, 2.0);

    use crate::client::test::dep0_2_0::test as v2;
    assert_eq!(v2::x(wrpc, ()).await?, 2.0);
    assert_eq!(v2::z(wrpc, (), 1.0, 1.0).await?, 4.0);
    Ok(())
}
