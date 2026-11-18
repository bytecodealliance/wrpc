use crate::client::test::options::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    option_none_param(&mut tx, &mut rx, None).await?;
    option_some_param(&mut tx, &mut rx, Some("foo")).await?;
    assert!(option_none_result(&mut tx, &mut rx).await?.is_none());
    assert_eq!(option_some_result(&mut tx, &mut rx).await?, Some("foo".to_string()));
    assert_eq!(
        option_roundtrip(&mut tx, &mut rx, Some("foo")).await?,
        Some("foo".to_string())
    );
    assert_eq!(
        double_option_roundtrip(&mut tx, &mut rx, Some(Some(42))).await?,
        Some(Some(42))
    );
    assert_eq!(
        double_option_roundtrip(&mut tx, &mut rx, Some(None)).await?,
        Some(None)
    );
    assert_eq!(double_option_roundtrip(&mut tx, &mut rx, None).await?, None);
    Ok(())
}
