use core::future::Future;
use core::pin::Pin;

use crate::server::exports::a::b::the_test::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(
        &self,
        future: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::std::io::Result<()> {
        future.await;
        Ok(())
    }
}
