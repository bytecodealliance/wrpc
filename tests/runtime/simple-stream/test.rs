use core::pin::Pin;

use futures::{Stream, StreamExt as _, stream};

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn read_stream(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Stream<Item = Vec<()>> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let mut x = x.flat_map(stream::iter);
        assert_eq!(x.by_ref().take(1).count().await, 1);
        assert_eq!(x.by_ref().take(2).count().await, 2);
        Ok(())
    }
}
