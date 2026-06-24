use crate::client::test::options::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    option_none_param(wrpc, (), None).await?;
    option_some_param(wrpc, (), Some("foo")).await?;
    assert!(option_none_result(wrpc, ()).await?.is_none());
    assert_eq!(option_some_result(wrpc, ()).await?, Some("foo".to_string()));
    assert_eq!(
        option_roundtrip(wrpc, (), Some("foo")).await?,
        Some("foo".to_string())
    );
    assert_eq!(
        double_option_roundtrip(wrpc, (), Some(Some(42))).await?,
        Some(Some(42))
    );
    assert_eq!(
        double_option_roundtrip(wrpc, (), Some(None)).await?,
        Some(None)
    );
    assert_eq!(double_option_roundtrip(wrpc, (), None).await?, None);
    Ok(())
}
