use crate::server::exports::my::inline::bar::Handler as HandlerBar;
use crate::server::exports::my::inline::foo::{Handler as HandlerFoo, HandlerA, A};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl HandlerFoo for Component {
    async fn bar(
        &self,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>> {
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::from_static(b"a"),
        ))
    }
}

impl HandlerA for Component {}

impl HandlerBar for Component {
    async fn bar(
        &self,
        m: ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<A>>,
    > {
        Ok(vec![m])
    }
}
