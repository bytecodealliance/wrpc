use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt as _, StreamExt as _, stream};

use crate::server::exports::my::test::test_interface::{
    Handler, HandlerTestThing, LeafThing, TestThing,
};

fn short_reads<T: ?Sized + Send + 'static>(
    s: BoxStream<'static, Vec<ResourceOwn<T>>>,
) -> BoxStream<'static, Vec<ResourceOwn<T>>> {
    // Read all the things, then write them all at once.  The caller will
    // read them only one at a time.
    Box::pin(stream::once(s.concat()))
}

fn dropped_reader<T: ?Sized + 'static>(
    f1: BoxFuture<'static, ResourceOwn<T>>,
    f2: BoxFuture<'static, ResourceOwn<T>>,
) -> (
    BoxFuture<'static, ResourceOwn<T>>,
    BoxFuture<'static, ResourceOwn<T>>,
) {
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
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn short_reads_test(
        &self,
        s: BoxStream<'static, Vec<ResourceOwn<TestThing>>>,
    ) -> ::std::io::Result<BoxStream<'static, Vec<ResourceOwn<TestThing>>>> {
        Ok(short_reads(s))
    }

    async fn short_reads_leaf(
        &self,
        s: BoxStream<'static, Vec<ResourceOwn<LeafThing>>>,
    ) -> ::std::io::Result<BoxStream<'static, Vec<ResourceOwn<LeafThing>>>> {
        Ok(short_reads(s))
    }

    async fn dropped_reader_test(
        &self,
        f1: BoxFuture<'static, ResourceOwn<TestThing>>,
        f2: BoxFuture<'static, ResourceOwn<TestThing>>,
    ) -> ::std::io::Result<(
        BoxFuture<'static, ResourceOwn<TestThing>>,
        BoxFuture<'static, ResourceOwn<TestThing>>,
    )> {
        Ok(dropped_reader(f1, f2))
    }

    async fn dropped_reader_leaf(
        &self,
        f1: BoxFuture<'static, ResourceOwn<LeafThing>>,
        f2: BoxFuture<'static, ResourceOwn<LeafThing>>,
    ) -> ::std::io::Result<(
        BoxFuture<'static, ResourceOwn<LeafThing>>,
        BoxFuture<'static, ResourceOwn<LeafThing>>,
    )> {
        Ok(dropped_reader(f1, f2))
    }
}

impl HandlerTestThing for Component {
    async fn new(
        &self,
        s: String,
    ) -> ::std::io::Result<ResourceOwn<TestThing>> {
        Ok(ResourceOwn::from(Bytes::from(s)))
    }

    async fn get(
        &self,
        self_: ResourceBorrow<TestThing>,
    ) -> ::std::io::Result<String> {
        let repr: &[u8] = self_.as_ref();
        let s = String::from_utf8(repr.to_vec()).map_err(std::io::Error::other)?;
        Ok(s)
    }
}
