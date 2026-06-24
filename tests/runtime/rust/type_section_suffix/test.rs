#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::bar::Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> crate::server::exports::foo::Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> crate::server::exports::test::suffix::imports::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
