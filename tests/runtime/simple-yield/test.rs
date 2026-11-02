use tokio::task::yield_now;

use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        yield_now().await;
        Ok(())
    }
}
