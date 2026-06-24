//@ args = '--generate-all'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::foo::baz::a::Handler<Ctx> for Component {
    async fn x(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
