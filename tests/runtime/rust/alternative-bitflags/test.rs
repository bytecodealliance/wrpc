//@ args = '--bitflags-path crate::test::my_bitflags'

pub(crate) use ::wit_bindgen_wrpc::bitflags as my_bitflags;

use crate::test::exports::my::inline::t::{Bar, Handler};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn get_flag(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Bar> {
        Ok(Bar::BAZ)
    }
}
