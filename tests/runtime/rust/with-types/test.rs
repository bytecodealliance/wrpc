use exports::my::inline::foo::{A, C};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::inline::foo::Handler<Ctx> for Component {
    async fn func1(&self, _cx: Ctx, v: A) -> anyhow::Result<A> {
        Ok(v)
    }
    async fn func3(&self, _cx: Ctx, v: C) -> anyhow::Result<C> {
        Ok(v)
    }
}
