//@ args = '--features y'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::foo::bar::bindings::Handler<Ctx> for Component {
    async fn y(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }

    async fn z(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
