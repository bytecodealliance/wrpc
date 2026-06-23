#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::inline::i::Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> anyhow::Result<()> {
        Ok(())
    }
    async fn one_argument(&self, _cx: Ctx, x: u32) -> anyhow::Result<()> {
        assert_eq!(x, 1);
        Ok(())
    }
    async fn one_result(&self, _cx: Ctx) -> anyhow::Result<u32> {
        Ok(2)
    }
    async fn one_argument_and_result(&self, _cx: Ctx, x: u32) -> anyhow::Result<u32> {
        assert_eq!(x, 3);
        Ok(4)
    }
}
