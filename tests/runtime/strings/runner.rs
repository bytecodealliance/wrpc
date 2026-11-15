use crate::client::test::strings::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    take_basic(tx, rx, "latin utf16").await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(return_unicode(tx, rx).await?, "🚀🚀🚀 𠈄𓀀");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(return_empty(tx, rx).await?, "");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip(tx, rx, "🚀🚀🚀 𠈄𓀀").await?, "🚀🚀🚀 𠈄𓀀");
    Ok(())
}
