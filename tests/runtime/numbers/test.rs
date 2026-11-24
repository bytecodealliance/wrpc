use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

#[derive(Clone)]
pub struct Component(pub crate::Client);

static SCALAR: AtomicU32 = AtomicU32::new(0);

impl crate::server::exports::test::numbers::numbers::Handler for Component {
    async fn roundtrip_u8(&self, a: u8) -> ::std::io::Result<u8> {
        Ok(a)
    }

    async fn roundtrip_s8(&self, a: i8) -> ::std::io::Result<i8> {
        Ok(a)
    }

    async fn roundtrip_u16(&self, a: u16) -> ::std::io::Result<u16> {
        Ok(a)
    }

    async fn roundtrip_s16(&self, a: i16) -> ::std::io::Result<i16> {
        Ok(a)
    }

    async fn roundtrip_u32(&self, a: u32) -> ::std::io::Result<u32> {
        Ok(a)
    }

    async fn roundtrip_s32(&self, a: i32) -> ::std::io::Result<i32> {
        Ok(a)
    }

    async fn roundtrip_u64(&self, a: u64) -> ::std::io::Result<u64> {
        Ok(a)
    }

    async fn roundtrip_s64(&self, a: i64) -> ::std::io::Result<i64> {
        Ok(a)
    }

    async fn roundtrip_f32(&self, a: f32) -> ::std::io::Result<f32> {
        Ok(a)
    }

    async fn roundtrip_f64(&self, a: f64) -> ::std::io::Result<f64> {
        Ok(a)
    }

    async fn roundtrip_char(&self, a: char) -> ::std::io::Result<char> {
        Ok(a)
    }

    async fn set_scalar(&self, val: u32) -> ::std::io::Result<()> {
        SCALAR.store(val, SeqCst);
        Ok(())
    }

    async fn get_scalar(&self) -> ::std::io::Result<u32> {
        Ok(SCALAR.load(SeqCst))
    }
}
