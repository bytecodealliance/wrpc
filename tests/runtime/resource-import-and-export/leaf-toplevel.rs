use ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn toplevel_export(
        &self,
        _cx: Ctx,
        a: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        Ok(a)
    }
}
