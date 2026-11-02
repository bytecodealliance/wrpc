use core::pin::Pin;

use ::wit_bindgen_wrpc::bytes::Bytes;
use futures::{Stream, StreamExt as _, stream};

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn read_stream(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        // Flatten the chunked stream into a stream of items
        let mut stream = x.flat_map(|buf| stream::iter(buf.to_vec()));
        // Read all items via StreamExt::next()
        assert_eq!(stream.next().await, Some(10));
        assert_eq!(stream.next().await, Some(20));
        assert_eq!(stream.next().await, Some(30));
        // Stream should be exhausted after the writer is dropped
        assert_eq!(stream.next().await, None);
        Ok(())
    }
}
