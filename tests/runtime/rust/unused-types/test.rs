//@ args = '--generate-unused-types'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::foo::bar::component::Handler<Ctx> for Component {
    async fn ping(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
}
