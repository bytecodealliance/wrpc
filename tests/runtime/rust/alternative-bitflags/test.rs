//@ args = '--bitflags-path=wit_bindgen_wrpc::bitflags'

use exports::my::inline::flags_iface::Bar;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::inline::flags_iface::Handler<Ctx> for Component {
    async fn get_flag(&self, _cx: Ctx) -> anyhow::Result<Bar> {
        Ok(Bar::BAZ)
    }
}
