use crate::client::test::options::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    option_none_param(tx, rx, None).await?;
    let (tx, rx) = wrpc.connect().await?;
    option_some_param(tx, rx, Some("foo")).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert!(option_none_result(tx, rx).await?.is_none());
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(option_some_result(tx, rx).await?, Some("foo".to_string()));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        option_roundtrip(tx, rx, Some("foo")).await?,
        Some("foo".to_string())
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        double_option_roundtrip(tx, rx, Some(Some(42))).await?,
        Some(Some(42))
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        double_option_roundtrip(tx, rx, Some(None)).await?,
        Some(None)
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(double_option_roundtrip(tx, rx, None).await?, None);
    Ok(())
}
