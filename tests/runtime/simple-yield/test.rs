use tokio::task::yield_now;

use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(&self) -> ::std::io::Result<()> {
        yield_now().await;
        Ok(())
    }
}
