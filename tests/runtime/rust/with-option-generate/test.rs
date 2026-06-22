//@ args = '--generate-all'

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::foo::baz::a::Handler<Ctx> for Component {
    async fn x(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
}
