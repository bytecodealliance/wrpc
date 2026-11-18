use ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn toplevel_export(
        &self,
        a: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        Ok(a)
    }
}
