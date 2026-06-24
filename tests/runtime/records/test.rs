use crate::test::exports::test::records::to_test::*;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::test::records::to_test::Handler<Ctx> for Component {
    async fn multiple_results(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<(u8, u16)> {
        Ok((4, 5))
    }

    async fn swap_tuple(
        &self,
        _cx: Ctx,
        a: (u8, u32),
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(u32, u8)> {
        Ok((a.1, a.0))
    }

    async fn roundtrip_flags1(&self, _cx: Ctx, a: F1) -> ::wit_bindgen_wrpc::anyhow::Result<F1> {
        Ok(a)
    }

    async fn roundtrip_flags2(&self, _cx: Ctx, a: F2) -> ::wit_bindgen_wrpc::anyhow::Result<F2> {
        Ok(a)
    }

    async fn roundtrip_flags3(
        &self,
        _cx: Ctx,
        a: Flag8,
        b: Flag16,
        c: Flag32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Flag8, Flag16, Flag32)> {
        Ok((a, b, c))
    }

    async fn roundtrip_record1(&self, _cx: Ctx, a: R1) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        Ok(a)
    }

    async fn tuple1(&self, _cx: Ctx, a: (u8,)) -> ::wit_bindgen_wrpc::anyhow::Result<(u8,)> {
        Ok((a.0,))
    }
}
