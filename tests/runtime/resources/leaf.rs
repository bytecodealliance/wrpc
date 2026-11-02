use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::imports::{Handler, HandlerY, Y};

static YS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn index(y: &[u8]) -> usize {
    usize::try_from(u32::from_le_bytes(y.try_into().unwrap())).unwrap()
}

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {}

impl<Ctx: Send> HandlerY<Ctx> for Component {
    async fn new(&self, _cx: Ctx, a: i32) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Y>> {
        let mut ys = YS.lock().unwrap();
        let idx = u32::try_from(ys.len())?;
        ys.push(a);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }

    async fn get_a(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<Y>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<i32> {
        Ok(YS.lock().unwrap()[index(self_.as_ref())])
    }

    async fn set_a(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<Y>,
        a: i32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        YS.lock().unwrap()[index(self_.as_ref())] = a;
        Ok(())
    }

    async fn add(
        &self,
        cx: Ctx,
        y: ResourceOwn<Y>,
        a: i32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Y>> {
        let a = YS.lock().unwrap()[index(y.as_ref())] + a;
        HandlerY::new(self, cx, a).await
    }
}
