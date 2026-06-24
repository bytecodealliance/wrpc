//@ args = '--bitflags-path crate::server::my_bitflags'

pub(crate) use ::wit_bindgen_wrpc::bitflags as my_bitflags;

use crate::server::exports::my::inline::t::{Bar, Handler};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn get_flag(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Bar> {
        Ok(Bar::BAZ)
    }
}
