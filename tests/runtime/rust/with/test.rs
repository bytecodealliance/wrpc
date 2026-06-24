#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::my::inline::bar::Handler<Ctx> for Component {
    async fn bar(
        &self,
        _cx: Ctx,
        m: crate::test::exports::my::inline::bar::Msg,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(m.field, "hello");
        Ok(())
    }
}
