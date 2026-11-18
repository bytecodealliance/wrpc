use core::future::Future;
use core::pin::Pin;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn take_then_drop(
        &self,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        drop(x);
        Ok(())
    }

    async fn read_and_drop(
        &self,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let _ = x.await;
        Ok(())
    }
}
