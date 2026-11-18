use tokio::task::yield_now;

use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        for _ in 0..10 {
            yield_now().await;
        }
        Ok(())
    }
}
