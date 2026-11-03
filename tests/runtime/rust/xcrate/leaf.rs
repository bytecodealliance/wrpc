use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::test::xcrate::{a_imports, b_imports};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> a_imports::Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> a_imports::HandlerX<Ctx> for Component {
    async fn new(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<a_imports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _cx: Ctx,
        _self_: ResourceBorrow<a_imports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> b_imports::Handler<Ctx> for Component {
    async fn b(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> b_imports::HandlerX<Ctx> for Component {
    async fn new(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<b_imports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _cx: Ctx,
        _self_: ResourceBorrow<b_imports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
