//@ args = '--features y'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::foo::bar::iface::Handler<Ctx> for Component {
    async fn y(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
    async fn z(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
}
