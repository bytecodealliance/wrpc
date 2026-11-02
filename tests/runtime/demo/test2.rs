use crate::server::exports::a::b::the_test::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn x(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
