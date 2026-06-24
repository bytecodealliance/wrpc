use crate::server::exports::my::inline::bar::Handler as HandlerBar;
use crate::server::exports::my::inline::foo::{Handler as HandlerFoo, HandlerA, A};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> HandlerFoo<Ctx> for Component {
    async fn bar(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>> {
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::from_static(b"a"),
        ))
    }
}

impl<Ctx: Send> HandlerA<Ctx> for Component {}

impl<Ctx: Send> HandlerBar<Ctx> for Component {
    async fn bar(
        &self,
        _cx: Ctx,
        m: ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>>,
    > {
        Ok(vec![m])
    }
}
