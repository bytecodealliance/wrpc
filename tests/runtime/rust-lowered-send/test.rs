use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn one_argument(&self, _cx: Ctx, x: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(&x, "hello");
        Ok(())
    }
}
