use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf_thing::exports::test::resource_import_and_export::test::{
    Handler, HandlerThing, Thing,
};

static THINGS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn index(thing: &[u8]) -> usize {
    usize::try_from(u32::from_le_bytes(thing.try_into().unwrap())).unwrap()
}

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {}

impl<Ctx: Send> HandlerThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        let mut things = THINGS.lock().unwrap();
        let idx = u32::try_from(things.len())?;
        things.push(v + 1);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }

    async fn foo(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(THINGS.lock().unwrap()[index(self_.as_ref())] + 2)
    }

    async fn bar(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<Thing>,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        THINGS.lock().unwrap()[index(self_.as_ref())] = v + 3;
        Ok(())
    }

    async fn baz(
        &self,
        cx: Ctx,
        a: ResourceOwn<Thing>,
        b: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        let v = {
            let things = THINGS.lock().unwrap();
            things[index(a.as_ref())] + 2 + things[index(b.as_ref())] + 2 + 4
        };
        HandlerThing::new(self, cx, v).await
    }
}
