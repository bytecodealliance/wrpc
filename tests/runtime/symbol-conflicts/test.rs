#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::my::inline::foo1::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> crate::test::exports::my::inline::foo2::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> crate::test::exports::my::inline::bar1::Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(String::new())
    }
}

impl<Ctx: Send> crate::test::exports::my::inline::bar2::Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(String::new())
    }
}
