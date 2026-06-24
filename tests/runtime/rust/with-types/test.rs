//@ args = '--generate-all'

use ::wit_bindgen_wrpc::anyhow::Result;
use ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn;

use crate::test::exports::i::{C as IC, D as ID, Handler as HandlerI};
use crate::test::exports::my::inline::bar::{E, Handler as HandlerBar};
use crate::test::exports::my::inline::foo::{
    A, B, F, G, Handler as HandlerFoo, HandlerB,
};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> HandlerFoo<Ctx> for Component {
    async fn func1(&self, _cx: Ctx, v: A) -> Result<A> {
        Ok(v)
    }
    async fn func2(&self, _cx: Ctx, v: ResourceOwn<B>) -> Result<ResourceOwn<B>> {
        Ok(v)
    }
    async fn func3(&self, _cx: Ctx, _v: Vec<A>) -> Result<Vec<ResourceOwn<B>>> {
        Ok(Vec::new())
    }
    async fn func4(&self, _cx: Ctx, v: Option<A>) -> Result<Option<A>> {
        Ok(v)
    }
    async fn func5(&self, _cx: Ctx) -> Result<core::result::Result<A, ()>> {
        Ok(Err(()))
    }
    async fn func6(&self, _cx: Ctx) -> Result<core::result::Result<F, ()>> {
        Ok(Err(()))
    }
    async fn func7(&self, _cx: Ctx) -> Result<core::result::Result<G, ()>> {
        Ok(Err(()))
    }
}

impl<Ctx: Send> HandlerB<Ctx> for Component {}

impl<Ctx: Send> HandlerBar<Ctx> for Component {
    async fn func6(&self, _cx: Ctx, v: E) -> Result<E> {
        Ok(v)
    }
}

impl<Ctx: Send> HandlerI<Ctx> for Component {
    async fn func7(&self, _cx: Ctx, v: IC) -> Result<IC> {
        Ok(v)
    }
    async fn func8(&self, _cx: Ctx, v: ID) -> Result<ID> {
        Ok(v)
    }
}
