use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::my::test::leaf_interface::{Handler, HandlerLeafThing, LeafThing};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerLeafThing for Component {
    async fn new(
        &self,
        s: String,
    ) -> ::std::io::Result<ResourceOwn<LeafThing>> {
        Ok(ResourceOwn::from(Bytes::from(s)))
    }

    async fn get(
        &self,
        self_: ResourceBorrow<LeafThing>,
    ) -> ::std::io::Result<String> {
        let repr: &[u8] = self_.as_ref();
        let s = String::from_utf8(repr.to_vec()).map_err(std::io::Error::other)?;
        Ok(s)
    }
}
