#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::a::b::x::Handler<Ctx> for Component {
    async fn f1(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> crate::test::exports::other::c::x::Handler<Ctx> for Component {
    async fn f2(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
