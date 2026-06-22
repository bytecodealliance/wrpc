use exports::my::inline::bar::Msg;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::inline::bar::Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx, m: Msg) -> anyhow::Result<()> {
        assert_eq!(m.field, "hello");
        Ok(())
    }
}
