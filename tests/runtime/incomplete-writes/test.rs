use core::future::Future;
use core::pin::Pin;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};
use futures::{FutureExt as _, Stream, StreamExt as _, stream};

use crate::server::exports::my::test::test_interface::{
    Handler, HandlerTestThing, LeafThing, TestThing,
};

type Things<T> = Pin<Box<dyn Stream<Item = Vec<ResourceOwn<T>>> + Send>>;
type Thing<T> = Pin<Box<dyn Future<Output = ResourceOwn<T>> + Send>>;

fn short_reads<T: ?Sized + Send + 'static>(s: Things<T>) -> Things<T> {
    // Read all the things, then write them all at once.  The caller will
    // read them only one at a time.
    Box::pin(stream::once(s.concat()))
}

fn dropped_reader<T: ?Sized + 'static>(f1: Thing<T>, f2: Thing<T>) -> (Thing<T>, Thing<T>) {
    // Drop the first future without reading from it.
    drop(f1);
    // Write the thing to the first future, the read end of which the
    // caller will drop without reading from, then write it again to the
    // second future, which the caller will read.
    let thing = f2.map(Bytes::from).shared();
    (
        Box::pin(thing.clone().map(ResourceOwn::from)),
        Box::pin(thing.map(ResourceOwn::from)),
    )
}

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn short_reads_test(
        &self,
        _cx: Ctx,
        s: Things<TestThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Things<TestThing>> {
        Ok(short_reads(s))
    }

    async fn short_reads_leaf(
        &self,
        _cx: Ctx,
        s: Things<LeafThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Things<LeafThing>> {
        Ok(short_reads(s))
    }

    async fn dropped_reader_test(
        &self,
        _cx: Ctx,
        f1: Thing<TestThing>,
        f2: Thing<TestThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Thing<TestThing>, Thing<TestThing>)> {
        Ok(dropped_reader(f1, f2))
    }

    async fn dropped_reader_leaf(
        &self,
        _cx: Ctx,
        f1: Thing<LeafThing>,
        f2: Thing<LeafThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Thing<LeafThing>, Thing<LeafThing>)> {
        Ok(dropped_reader(f1, f2))
    }
}

impl<Ctx: Send> HandlerTestThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        s: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<TestThing>> {
        Ok(ResourceOwn::from(Bytes::from(s)))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<TestThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        let repr: &[u8] = self_.as_ref();
        let s = String::from_utf8(repr.to_vec())?;
        Ok(s)
    }
}
