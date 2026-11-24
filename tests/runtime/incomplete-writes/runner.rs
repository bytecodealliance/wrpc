//@ wasmtime-flags = '-Wcomponent-model-async'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::ResourceOwn;
use futures::{StreamExt as _, future, stream};
use tokio::try_join;

use crate::client::my::test::leaf_interface::LeafThing;
use crate::client::my::test::test_interface::{
    TestThing, dropped_reader_leaf, dropped_reader_test, short_reads_leaf, short_reads_test,
};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let a = TestThing::new(&mut clt, "a").await?;
        let b = TestThing::new(&mut clt, "b").await?;
        let c = TestThing::new(&mut clt, "c").await?;
        let things = vec![a, b, c];
        let (stream, io) =
            short_reads_test(&mut clt, Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, io)?;
        assert_eq!(things.len(), 3);
        let a = TestThing::get(&mut clt, &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let b = TestThing::get(&mut clt, &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let c = TestThing::get(&mut clt, &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let a = LeafThing::new(&mut clt, "a").await?;
        let b = LeafThing::new(&mut clt, "b").await?;
        let c = LeafThing::new(&mut clt, "c").await?;
        let things = vec![a, b, c];
        let (stream, io) =
            short_reads_leaf(&mut clt, Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, io)?;
        assert_eq!(things.len(), 3);
        let a = LeafThing::get(&mut clt, &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let b = LeafThing::get(&mut clt, &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let c = LeafThing::get(&mut clt, &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let thing = TestThing::new(&mut clt, "a").await?;
        let thing = Bytes::from(thing);
        let ((.., f2), io) = dropped_reader_test(
            &mut clt,
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, io)?;
        let a = TestThing::get(&mut clt, &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let thing = LeafThing::new(&mut clt, "a").await?;
        let thing = Bytes::from(thing);
        let ((.., f2), io) = dropped_reader_leaf(
            &mut clt,
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, io)?;
        let a = LeafThing::get(&mut clt, &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }
    Ok(())
}
