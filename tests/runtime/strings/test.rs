#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::test::strings::to_test::Handler<Ctx> for Component {
    async fn take_basic(&self, _cx: Ctx, s: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(s, "latin utf16");
        Ok(())
    }

    async fn return_unicode(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("🚀🚀🚀 𠈄𓀀".to_string())
    }

    async fn return_empty(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("".to_string())
    }

    async fn roundtrip(&self, _cx: Ctx, s: String) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(s.clone())
    }
}
