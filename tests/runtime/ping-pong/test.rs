use core::future::Future;
use core::pin::Pin;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn ping(
        &self,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
        y: String,
    ) -> ::std::io::Result<Pin<Box<dyn Future<Output = String> + Send>>> {
        let msg = x.await + y.as_str();
        Ok(Box::pin(async move { msg }))
    }

    async fn pong(
        &self,
        x: Pin<Box<dyn Future<Output = String> + Send>>,
    ) -> ::std::io::Result<String> {
        Ok(x.await)
    }
}
