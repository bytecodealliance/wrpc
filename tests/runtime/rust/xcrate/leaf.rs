use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::test::xcrate::{a_imports, b_imports};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl a_imports::Handler for Component {
    async fn f(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl a_imports::HandlerX for Component {
    async fn new(&self) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<a_imports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _self_: ResourceBorrow<a_imports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl b_imports::Handler for Component {
    async fn b(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl b_imports::HandlerX for Component {
    async fn new(&self) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<b_imports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _self_: ResourceBorrow<b_imports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
