use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

#[derive(Clone)]
pub struct Component;

static SCALAR: AtomicU32 = AtomicU32::new(0);

impl<Ctx: Send> crate::server::exports::test::numbers::numbers::Handler<Ctx> for Component {
    async fn roundtrip_u8(&self, _cx: Ctx, a: u8) -> ::wit_bindgen_wrpc::anyhow::Result<u8> {
        Ok(a)
    }

    async fn roundtrip_s8(&self, _cx: Ctx, a: i8) -> ::wit_bindgen_wrpc::anyhow::Result<i8> {
        Ok(a)
    }

    async fn roundtrip_u16(&self, _cx: Ctx, a: u16) -> ::wit_bindgen_wrpc::anyhow::Result<u16> {
        Ok(a)
    }

    async fn roundtrip_s16(&self, _cx: Ctx, a: i16) -> ::wit_bindgen_wrpc::anyhow::Result<i16> {
        Ok(a)
    }

    async fn roundtrip_u32(&self, _cx: Ctx, a: u32) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(a)
    }

    async fn roundtrip_s32(&self, _cx: Ctx, a: i32) -> ::wit_bindgen_wrpc::anyhow::Result<i32> {
        Ok(a)
    }

    async fn roundtrip_u64(&self, _cx: Ctx, a: u64) -> ::wit_bindgen_wrpc::anyhow::Result<u64> {
        Ok(a)
    }

    async fn roundtrip_s64(&self, _cx: Ctx, a: i64) -> ::wit_bindgen_wrpc::anyhow::Result<i64> {
        Ok(a)
    }

    async fn roundtrip_f32(&self, _cx: Ctx, a: f32) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(a)
    }

    async fn roundtrip_f64(&self, _cx: Ctx, a: f64) -> ::wit_bindgen_wrpc::anyhow::Result<f64> {
        Ok(a)
    }

    async fn roundtrip_char(&self, _cx: Ctx, a: char) -> ::wit_bindgen_wrpc::anyhow::Result<char> {
        Ok(a)
    }

    async fn set_scalar(&self, _cx: Ctx, val: u32) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        SCALAR.store(val, SeqCst);
        Ok(())
    }

    async fn get_scalar(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(SCALAR.load(SeqCst))
    }
}
