//@ args = '--skip foo'

use crate::server::exports::exports::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn bar(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
