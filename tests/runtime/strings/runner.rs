use crate::client::test::strings::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    take_basic(&mut clt, "latin utf16").await?;
    assert_eq!(return_unicode(&mut clt).await?, "🚀🚀🚀 𠈄𓀀");
    assert_eq!(return_empty(&mut clt).await?, "");
    assert_eq!(roundtrip(&mut clt, "🚀🚀🚀 𠈄𓀀").await?, "🚀🚀🚀 𠈄𓀀");
    Ok(())
}
