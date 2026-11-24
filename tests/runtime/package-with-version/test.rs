#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::my::inline::foo::Handler for Component {}

impl crate::server::exports::my::inline::foo::HandlerBar for Component {
    async fn new(
        &self,
    ) -> ::std::io::Result<
        ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<
            crate::server::exports::my::inline::foo::Bar,
        >,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_core::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::new(),
        ))
    }
}
