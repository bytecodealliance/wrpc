use core::future::Future;
use core::pin::Pin;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn pending_import(
        &self,
        x: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        x.await;
        Ok(())
    }

    async fn backpressure_set(&self, _x: bool) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
