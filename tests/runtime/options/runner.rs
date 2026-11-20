use crate::client::test::options::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    option_none_param(&mut clt, None).await?;
    option_some_param(&mut clt, Some("foo")).await?;
    assert!(option_none_result(&mut clt).await?.is_none());
    assert_eq!(option_some_result(&mut clt).await?, Some("foo".to_string()));
    assert_eq!(
        option_roundtrip(&mut clt, Some("foo")).await?,
        Some("foo".to_string())
    );
    assert_eq!(
        double_option_roundtrip(&mut clt, Some(Some(42))).await?,
        Some(Some(42))
    );
    assert_eq!(
        double_option_roundtrip(&mut clt, Some(None)).await?,
        Some(None)
    );
    assert_eq!(double_option_roundtrip(&mut clt, None).await?, None);
    Ok(())
}
