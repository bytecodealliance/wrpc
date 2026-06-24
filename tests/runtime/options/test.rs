#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::test::options::to_test::Handler<Ctx> for Component {
    async fn option_none_param(
        &self,
        _cx: Ctx,
        a: Option<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert!(a.is_none());
        Ok(())
    }

    async fn option_none_result(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn option_some_param(
        &self,
        _cx: Ctx,
        a: Option<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(a, Some("foo".to_string()));
        Ok(())
    }

    async fn option_some_result(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<String>> {
        Ok(Some("foo".to_string()))
    }

    async fn option_roundtrip(
        &self,
        _cx: Ctx,
        a: Option<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<String>> {
        Ok(a)
    }

    async fn double_option_roundtrip(
        &self,
        _cx: Ctx,
        a: Option<Option<u32>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<Option<u32>>> {
        Ok(a)
    }
}
