use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::test::resource_with_lists::test::{Handler, HandlerThing, Thing};

static THINGS: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

fn index(thing: &ResourceBorrow<Thing>) -> usize {
    let bytes: &[u8] = thing.as_ref();
    usize::try_from(u32::from_le_bytes(bytes.try_into().unwrap())).unwrap()
}

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerThing for Component {
    async fn new(
        &self,
        l: Bytes,
    ) -> ::std::io::Result<ResourceOwn<Thing>> {
        let mut l = l.to_vec();
        l.extend_from_slice(" HostThing".as_bytes());
        let mut things = THINGS.lock().unwrap();
        let idx = u32::try_from(things.len()).map_err(std::io::Error::other)?;
        things.push(l);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }

    async fn foo(
        &self,
        self_: ResourceBorrow<Thing>,
    ) -> ::std::io::Result<Bytes> {
        let mut list = THINGS.lock().unwrap()[index(&self_)].clone();
        list.extend_from_slice(" HostThing.foo".as_bytes());
        Ok(list.into())
    }

    async fn bar(
        &self,
        self_: ResourceBorrow<Thing>,
        l: Bytes,
    ) -> ::std::io::Result<()> {
        let mut l = l.to_vec();
        l.extend_from_slice(" HostThing.bar".as_bytes());
        THINGS.lock().unwrap()[index(&self_)] = l;
        Ok(())
    }

    async fn baz(&self, l: Bytes) -> ::std::io::Result<Bytes> {
        let mut l = l.to_vec();
        l.extend_from_slice(" HostThing.baz".as_bytes());
        Ok(l.into())
    }
}
