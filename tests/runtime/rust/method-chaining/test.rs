//@ args = '--enable-method-chaining'

use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::server::exports::foo::bar::i::{A, Handler, HandlerA};

static PROPS: Mutex<(u32, bool)> = Mutex::new((0, false));

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerA for Component {
    async fn new(&self) -> ::std::io::Result<ResourceOwn<A>> {
        Ok(ResourceOwn::from(Bytes::from_static(b"a")))
    }

    async fn set_a(
        &self,
        _self_: ResourceBorrow<A>,
        a: u32,
    ) -> ::std::io::Result<()> {
        PROPS.lock().unwrap().0 = a;
        Ok(())
    }

    async fn set_b(
        &self,
        _self_: ResourceBorrow<A>,
        b: bool,
    ) -> ::std::io::Result<()> {
        PROPS.lock().unwrap().1 = b;
        Ok(())
    }

    async fn do_(
        &self,
        _self_: ResourceBorrow<A>,
    ) -> ::std::io::Result<()> {
        Ok(())
    }
}
