use crate::server::exports::test::resource_alias::e1;
use crate::server::exports::test::resource_alias::e2;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl e1::Handler for Component {
    async fn a(
        &self,
        f: e1::Foo,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<e1::X>>,
    > {
        Ok(vec![f.x])
    }
}

impl e1::HandlerX for Component {
    async fn new(
        &self,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<e1::X>,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(&v.to_le_bytes()),
        ))
    }
}

impl e2::Handler for Component {
    async fn a(
        &self,
        f: e2::Foo,
        g: e2::Bar,
        _h: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<e2::Y>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<e2::Y>>,
    > {
        Ok(vec![f.x, g.x])
    }
}
