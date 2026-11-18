use tokio::sync::Notify;

use crate::server::exports::my::test::i::Handler;

static NOTIFY: Notify = Notify::const_new();

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn pending_import(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        NOTIFY.notified().await;
        Ok(())
    }

    async fn resolve_pending_import(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        NOTIFY.notify_one();
        Ok(())
    }
}
