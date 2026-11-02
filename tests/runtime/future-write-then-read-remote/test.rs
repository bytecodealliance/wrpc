use core::future::Future;
use core::pin::Pin;

use crate::server::exports::a::b::the_test::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn f(
        &self,
        _cx: Ctx,
        future: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        future.await;
        Ok(())
    }
}
