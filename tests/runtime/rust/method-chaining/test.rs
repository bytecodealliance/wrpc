//@ args = '--enable-method-chaining'

use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::foo::bar::i::{A, Handler, HandlerA};

static PROPS: Mutex<(u32, bool)> = Mutex::new((0, false));

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {}

impl<Ctx: Send> HandlerA<Ctx> for Component {
    async fn new(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<A>> {
        Ok(ResourceOwn::from(Bytes::from_static(b"a")))
    }

    async fn set_a(
        &self,
        _cx: Ctx,
        _self_: ResourceBorrow<A>,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        PROPS.lock().unwrap().0 = a;
        Ok(())
    }

    async fn set_b(
        &self,
        _cx: Ctx,
        _self_: ResourceBorrow<A>,
        b: bool,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        PROPS.lock().unwrap().1 = b;
        Ok(())
    }

    async fn do_(
        &self,
        _cx: Ctx,
        _self_: ResourceBorrow<A>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
