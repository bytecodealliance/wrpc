//@ wasmtime-flags = '-Wcomponent-model-async'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn;
use futures::{StreamExt as _, future, stream};
use tokio::try_join;

use crate::client::my::test::leaf_interface::LeafThing;
use crate::client::my::test::test_interface::{
    TestThing, dropped_reader_leaf, dropped_reader_test, short_reads_leaf, short_reads_test,
};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let (tx, rx) = wrpc.connect().await?;
        let a = TestThing::new(tx, rx, "a").await?;
        let (tx, rx) = wrpc.connect().await?;
        let b = TestThing::new(tx, rx, "b").await?;
        let (tx, rx) = wrpc.connect().await?;
        let c = TestThing::new(tx, rx, "c").await?;
        let things = vec![a, b, c];
        let (tx, rx) = wrpc.connect().await?;
        let (stream, io) = short_reads_test(tx, rx, Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, io)?;
        assert_eq!(things.len(), 3);
        let (tx, rx) = wrpc.connect().await?;
        let a = TestThing::get(tx, rx, &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let (tx, rx) = wrpc.connect().await?;
        let b = TestThing::get(tx, rx, &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let (tx, rx) = wrpc.connect().await?;
        let c = TestThing::get(tx, rx, &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let (tx, rx) = wrpc.connect().await?;
        let a = LeafThing::new(tx, rx, "a").await?;
        let (tx, rx) = wrpc.connect().await?;
        let b = LeafThing::new(tx, rx, "b").await?;
        let (tx, rx) = wrpc.connect().await?;
        let c = LeafThing::new(tx, rx, "c").await?;
        let things = vec![a, b, c];
        let (tx, rx) = wrpc.connect().await?;
        let (stream, io) = short_reads_leaf(tx, rx, Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, io)?;
        assert_eq!(things.len(), 3);
        let (tx, rx) = wrpc.connect().await?;
        let a = LeafThing::get(tx, rx, &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let (tx, rx) = wrpc.connect().await?;
        let b = LeafThing::get(tx, rx, &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let (tx, rx) = wrpc.connect().await?;
        let c = LeafThing::get(tx, rx, &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let (tx, rx) = wrpc.connect().await?;
        let thing = TestThing::new(tx, rx, "a").await?;
        let thing = Bytes::from(thing);
        let (tx, rx) = wrpc.connect().await?;
        let ((.., f2), io) = dropped_reader_test(
            tx,
            rx,
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, io)?;
        let (tx, rx) = wrpc.connect().await?;
        let a = TestThing::get(tx, rx, &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let (tx, rx) = wrpc.connect().await?;
        let thing = LeafThing::new(tx, rx, "a").await?;
        let thing = Bytes::from(thing);
        let (tx, rx) = wrpc.connect().await?;
        let ((.., f2), io) = dropped_reader_leaf(
            tx,
            rx,
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, io)?;
        let (tx, rx) = wrpc.connect().await?;
        let a = LeafThing::get(tx, rx, &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }
    Ok(())
}
