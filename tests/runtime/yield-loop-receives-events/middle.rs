use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use crate::server::exports::test::common::i_runner::Handler;
use crate::server::test::common::i_middle::f;

static HIT: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let clt = self.0;
        let task = tokio::spawn(async move {
            let clt = clt.connect().await?;
            f(clt).await?;
            HIT.store(true, Ordering::Relaxed);
            ::wit_bindgen_wrpc::anyhow::Ok(())
        });
        // This is an "infinite loop" but it's also effectively a yield which
        // should enable not only making progress on sibling rust-level tasks
        // but additionally async events should be deliverable.
        poll_fn(|cx| {
            if HIT.load(Ordering::Relaxed) || task.is_finished() {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        task.await?
    }
}
