use crate::test::exports::test::resource_borrow::to_test::{Handler, HandlerThing, Thing};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn foo(
        &self,
        _cx: Ctx,
        v: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let bytes: &[u8] = v.as_ref();
        let val = u32::from_le_bytes(bytes.try_into().unwrap());
        Ok(val + 2)
    }
}

impl<Ctx: Send> HandlerThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>,
    > {
        let val = v + 1;
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(&val.to_le_bytes()),
        ))
    }
}
