use crate::server::exports::test::resource_into_inner::to_test::{Handler, HandlerThing, Thing};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn test(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl HandlerThing for Component {
    async fn new(
        &self,
        text: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(text.as_bytes()),
        ))
    }
}
