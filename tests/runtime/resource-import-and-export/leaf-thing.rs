use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::leaf_thing::exports::test::resource_import_and_export::test::{
    Handler, HandlerThing, Thing,
};

static THINGS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn index(thing: &[u8]) -> usize {
    usize::try_from(u32::from_le_bytes(thing.try_into().unwrap())).unwrap()
}

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerThing for Component {
    async fn new(
        &self,
        v: u32,
    ) -> ::std::io::Result<ResourceOwn<Thing>> {
        let mut things = THINGS.lock().unwrap();
        let idx = u32::try_from(things.len()).map_err(std::io::Error::other)?;
        things.push(v + 1);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }

    async fn foo(
        &self,
        self_: ResourceBorrow<Thing>,
    ) -> ::std::io::Result<u32> {
        Ok(THINGS.lock().unwrap()[index(self_.as_ref())] + 2)
    }

    async fn bar(
        &self,
        self_: ResourceBorrow<Thing>,
        v: u32,
    ) -> ::std::io::Result<()> {
        THINGS.lock().unwrap()[index(self_.as_ref())] = v + 3;
        Ok(())
    }

    async fn baz(
        &self,
        a: ResourceOwn<Thing>,
        b: ResourceOwn<Thing>,
    ) -> ::std::io::Result<ResourceOwn<Thing>> {
        let v = {
            let things = THINGS.lock().unwrap();
            things[index(a.as_ref())] + 2 + things[index(b.as_ref())] + 2 + 4
        };
        HandlerThing::new(self, v).await
    }
}
