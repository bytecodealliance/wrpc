use crate::server::exports::test::flags_widths::to_test::*;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::test::flags_widths::to_test::Handler<Ctx> for Component {
    async fn roundtrip_flag17(
        &self,
        _cx: Ctx,
        a: Flag17,
        b: u8,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Flag17, u8)> {
        Ok((a, b))
    }

    async fn roundtrip_flag40(
        &self,
        _cx: Ctx,
        a: Flag40,
        b: u8,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Flag40, u8)> {
        Ok((a, b))
    }
}
