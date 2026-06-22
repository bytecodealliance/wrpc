//@ args = '--skip foo'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::test::exports_iface::Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
}
