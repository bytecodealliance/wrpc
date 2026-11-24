use crate::server::exports::test::resource_borrow::to_test::{Handler, HandlerThing, Thing};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn foo(
        &self,
        v: ::wit_bindgen_wrpc::wrpc_core::ResourceBorrow<Thing>,
    ) -> ::std::io::Result<u32> {
        let bytes: &[u8] = v.as_ref();
        let val = u32::from_le_bytes(bytes.try_into().unwrap());
        Ok(val + 2)
    }
}

impl HandlerThing for Component {
    async fn new(
        &self,
        v: u32,
    ) -> ::std::io::Result<
        ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Thing>,
    > {
        let val = v + 1;
        Ok(::wit_bindgen_wrpc::wrpc_core::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(&val.to_le_bytes()),
        ))
    }
}
