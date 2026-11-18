use core::future::Future;
use core::pin::Pin;

use crate::server::exports::a::b::the_test::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(
        &self,
        future: Pin<Box<dyn Future<Output = u8> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(future.await, 0);
        Ok(())
    }
}
