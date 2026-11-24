use tokio::task::yield_now;

use crate::leaf::exports::test::common::i_middle::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(&self) -> ::std::io::Result<()> {
        for _ in 0..2 {
            yield_now().await;
        }
        Ok(())
    }
}
