//@ args = '--generate-unused-types'

#[expect(unused_imports)]
use crate::server::exports::foo::bar::component::UnusedEnum as _;
#[expect(unused_imports)]
use crate::server::exports::foo::bar::component::UnusedRecord as _;
#[expect(unused_imports)]
use crate::server::exports::foo::bar::component::UnusedVariant as _;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::foo::bar::component::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
