#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::cat::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx, x: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(x, "hello");
        Ok(())
    }

    async fn bar(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("world".into())
    }
}
