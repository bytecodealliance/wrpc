use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn;

use crate::server::exports::test::ownership::both_list_and_resource;
use crate::server::exports::{lists, thing_in, thing_in_and_out};

static RESOURCES: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> lists::Handler<Ctx> for Component {
    async fn foo(
        &self,
        _cx: Ctx,
        list: Vec<Vec<String>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Vec<Vec<String>>> {
        Ok(list)
    }
}

impl<Ctx: Send> thing_in::Handler<Ctx> for Component {
    async fn bar(
        &self,
        _cx: Ctx,
        _value: thing_in::Thing,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> thing_in_and_out::Handler<Ctx> for Component {
    async fn baz(
        &self,
        _cx: Ctx,
        value: thing_in_and_out::Thing,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<thing_in_and_out::Thing> {
        Ok(value)
    }
}

impl<Ctx: Send> both_list_and_resource::Handler<Ctx> for Component {
    async fn list_and_resource(
        &self,
        _cx: Ctx,
        value: both_list_and_resource::Thing,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let idx: &[u8] = value.b.as_ref();
        let idx: [u8; 4] = idx.try_into()?;
        let idx = u32::from_le_bytes(idx);
        let idx = usize::try_from(idx)?;
        assert_eq!(value.a, RESOURCES.lock().unwrap()[idx]);
        Ok(())
    }
}

impl<Ctx: Send> both_list_and_resource::HandlerTheResource<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        list: Vec<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<both_list_and_resource::TheResource>> {
        let mut resources = RESOURCES.lock().unwrap();
        let idx = u32::try_from(resources.len())?;
        resources.push(list);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }
}
