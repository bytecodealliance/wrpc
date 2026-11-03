use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn return_string(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("hello".into())
    }
}
