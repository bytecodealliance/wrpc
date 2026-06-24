#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::my::inline::foo::Handler<Ctx> for Component {}

impl<Ctx: Send> crate::test::exports::my::inline::foo::HandlerBar<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<
            crate::test::exports::my::inline::foo::Bar,
        >,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::new(),
        ))
    }
}
