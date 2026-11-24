use core::pin::Pin;

use ::wit_bindgen_wrpc::bytes::Bytes;
use futures::{Stream, StreamExt as _, stream};

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn read_stream(
        &self,
        x: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> ::std::io::Result<()> {
        let mut x = x.flat_map(|buf| stream::iter(buf.to_vec()));
        // read one item
        assert_eq!(x.by_ref().take(1).collect::<Vec<_>>().await, [0]);
        // read two items
        assert_eq!(x.by_ref().take(2).collect::<Vec<_>>().await, [1, 2]);
        // read 1/2 items
        assert_eq!(x.by_ref().take(1).collect::<Vec<_>>().await, [3]);
        // read the next buffered item
        assert_eq!(x.by_ref().take(1).collect::<Vec<_>>().await, [4]);
        // drop
        drop(x);
        Ok(())
    }
}
