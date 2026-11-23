use crate::server::exports::my::inline::foo::{Handler, HandlerBar, Bar};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerBar for Component {
    async fn new(
        &self,
        data: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Bar>,
    > {
        Ok(::wit_bindgen_wrpc::wrpc_core::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(&data.to_le_bytes()),
        ))
    }

    async fn get_data(
        &self,
        self_: ::wit_bindgen_wrpc::wrpc_core::ResourceBorrow<Bar>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let bytes: &[u8] = self_.as_ref();
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    async fn consume(
        &self,
        self_: ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Bar>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let bytes: &[u8] = self_.as_ref();
        let data = u32::from_le_bytes(bytes.try_into().unwrap());
        Ok(data + 1)
    }
}
