use core::future::Future;
use core::pin::Pin;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn read_future(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        x.await;
        Ok(())
    }

    async fn drop_future(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        drop(x);
        Ok(())
    }
}
