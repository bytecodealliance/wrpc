use crate::client::test::strings::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    take_basic(&mut tx, &mut rx, "latin utf16").await?;
    assert_eq!(return_unicode(&mut tx, &mut rx).await?, "🚀🚀🚀 𠈄𓀀");
    assert_eq!(return_empty(&mut tx, &mut rx).await?, "");
    assert_eq!(roundtrip(&mut tx, &mut rx, "🚀🚀🚀 𠈄𓀀").await?, "🚀🚀🚀 𠈄𓀀");
    Ok(())
}
