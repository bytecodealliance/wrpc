use crate::server::exports::test::resource_into_inner::to_test::{Handler, HandlerThing, Thing};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn test(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}

impl HandlerThing for Component {
    async fn new(
        &self,
        text: String,
    ) -> ::std::io::Result<
        ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Thing>,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_core::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(text.as_bytes()),
        ))
    }
}
