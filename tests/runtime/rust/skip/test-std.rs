//@ args = '--skip foo'

use crate::server::exports::exports::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn bar(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
