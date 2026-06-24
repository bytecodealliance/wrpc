use crate::runner::test::strings::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    take_basic(wrpc, (), "latin utf16").await?;
    assert_eq!(return_unicode(wrpc, ()).await?, "🚀🚀🚀 𠈄𓀀");
    assert_eq!(return_empty(wrpc, ()).await?, "");
    assert_eq!(roundtrip(wrpc, (), "🚀🚀🚀 𠈄𓀀").await?, "🚀🚀🚀 𠈄𓀀");
    Ok(())
}
