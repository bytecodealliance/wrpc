use core::future::Future;
use core::pin::Pin;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn ping(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
        y: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Pin<Box<dyn Future<Output = String> + Send>>> {
        let msg = x.await + y.as_str();
        Ok(Box::pin(async move { msg }))
    }

    async fn pong(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(x.await)
    }
}
