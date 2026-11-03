use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::my::test::leaf_interface::{Handler, HandlerLeafThing, LeafThing};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {}

impl<Ctx: Send> HandlerLeafThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        s: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<LeafThing>> {
        Ok(ResourceOwn::from(Bytes::from(s)))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<LeafThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        let repr: &[u8] = self_.as_ref();
        let s = String::from_utf8(repr.to_vec())?;
        Ok(s)
    }
}
