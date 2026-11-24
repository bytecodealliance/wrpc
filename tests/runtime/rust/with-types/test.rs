//@ args = '--generate-all'

use ::std::io::Result;
use ::wit_bindgen_wrpc::wrpc_core::ResourceOwn;

use crate::server::exports::i::{C as IC, D as ID, Handler as HandlerI};
use crate::server::exports::my::inline::bar::{E, Handler as HandlerBar};
use crate::server::exports::my::inline::foo::{
    A, B, F, G, Handler as HandlerFoo, HandlerB,
};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl HandlerFoo for Component {
    async fn func1(&self, v: A) -> Result<A> {
        Ok(v)
    }
    async fn func2(&self, v: ResourceOwn<B>) -> Result<ResourceOwn<B>> {
        Ok(v)
    }
    async fn func3(&self, _v: Vec<A>) -> Result<Vec<ResourceOwn<B>>> {
        Ok(Vec::new())
    }
    async fn func4(&self, v: Option<A>) -> Result<Option<A>> {
        Ok(v)
    }
    async fn func5(&self) -> Result<core::result::Result<A, ()>> {
        Ok(Err(()))
    }
    async fn func6(&self) -> Result<core::result::Result<F, ()>> {
        Ok(Err(()))
    }
    async fn func7(&self) -> Result<core::result::Result<G, ()>> {
        Ok(Err(()))
    }
}

impl HandlerB for Component {}

impl HandlerBar for Component {
    async fn func6(&self, v: E) -> Result<E> {
        Ok(v)
    }
}

impl HandlerI for Component {
    async fn func7(&self, v: IC) -> Result<IC> {
        Ok(v)
    }
    async fn func8(&self, v: ID) -> Result<ID> {
        Ok(v)
    }
}
