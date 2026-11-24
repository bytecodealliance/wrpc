use ::wit_bindgen_wrpc::wrpc_core::ResourceOwn;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn toplevel_export(
        &self,
        a: ResourceOwn<Thing>,
    ) -> ::std::io::Result<ResourceOwn<Thing>> {
        Ok(a)
    }
}
