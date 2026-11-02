use tokio::task::yield_now;

use crate::leaf::exports::test::common::i_middle::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        for _ in 0..2 {
            yield_now().await;
        }
        Ok(())
    }
}
