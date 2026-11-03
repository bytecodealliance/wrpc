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
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let a = TestThing::new(wrpc, (), "a").await?;
        let b = TestThing::new(wrpc, (), "b").await?;
        let c = TestThing::new(wrpc, (), "c").await?;
        let things = vec![a, b, c];
        let (stream, io) = short_reads_test(wrpc, (), Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, async {
            match io {
                Some(io) => io.await,
                None => Ok(()),
            }
        })?;
        assert_eq!(things.len(), 3);
        let a = TestThing::get(wrpc, (), &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let b = TestThing::get(wrpc, (), &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let c = TestThing::get(wrpc, (), &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write the things all at once.  The callee will read them only
        // one at a time.
        let a = LeafThing::new(wrpc, (), "a").await?;
        let b = LeafThing::new(wrpc, (), "b").await?;
        let c = LeafThing::new(wrpc, (), "c").await?;
        let things = vec![a, b, c];
        let (stream, io) = short_reads_leaf(wrpc, (), Box::pin(stream::iter([things]))).await?;
        let (things, ()) = try_join!(async { Ok(stream.concat().await) }, async {
            match io {
                Some(io) => io.await,
                None => Ok(()),
            }
        })?;
        assert_eq!(things.len(), 3);
        let a = LeafThing::get(wrpc, (), &things[0].as_borrow()).await?;
        assert_eq!(a, "a");
        let b = LeafThing::get(wrpc, (), &things[1].as_borrow()).await?;
        assert_eq!(b, "b");
        let c = LeafThing::get(wrpc, (), &things[2].as_borrow()).await?;
        assert_eq!(c, "c");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let thing = TestThing::new(wrpc, (), "a").await?;
        let thing = Bytes::from(thing);
        let ((.., f2), io) = dropped_reader_test(
            wrpc,
            (),
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, async {
            match io {
                Some(io) => io.await,
                None => Ok(()),
            }
        })?;
        let a = TestThing::get(wrpc, (), &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }

    {
        // Write a thing to the first future, the read end of which the
        // callee will drop without reading from, then write it again to
        // the second future, which the callee will read.
        let thing = LeafThing::new(wrpc, (), "a").await?;
        let thing = Bytes::from(thing);
        let ((.., f2), io) = dropped_reader_leaf(
            wrpc,
            (),
            Box::pin(future::ready(ResourceOwn::from(thing.clone()))),
            Box::pin(future::ready(ResourceOwn::from(thing))),
        )
        .await?;
        // Drop the first future without reading from it and read from
        // the second future and assert it matches what we wrote above.
        let (thing, ()) = try_join!(async { Ok(f2.await) }, async {
            match io {
                Some(io) => io.await,
                None => Ok(()),
            }
        })?;
        let a = LeafThing::get(wrpc, (), &thing.as_borrow()).await?;
        assert_eq!(a, "a");
    }
    Ok(())
}
