//@ wasmtime-flags = '-Wcomponent-model-fixed-length-lists'

use crate::server::exports::test::fixed_length_lists::to_test::{Handler, Nested};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn list_param(&self, _cx: Ctx, a: [u32; 4]) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(a, [1, 2, 3, 4]);
        Ok(())
    }

    async fn list_param2(
        &self,
        _cx: Ctx,
        a: [[u32; 2]; 2],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(a, [[1, 2], [3, 4]]);
        Ok(())
    }

    async fn list_param3(&self, _cx: Ctx, a: [i32; 20]) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(
            a,
            [-1, 2, -3, 4, -5, 6, -7, 8, -9, 10, -11, 12, -13, 14, -15, 16, -17, 18, -19, 20]
        );
        Ok(())
    }

    async fn list_minmax16(
        &self,
        _cx: Ctx,
        a: [u16; 4],
        b: [i16; 4],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<([u16; 4], [i16; 4])> {
        Ok((a, b))
    }

    async fn list_minmax_float(
        &self,
        _cx: Ctx,
        a: [f32; 2],
        b: [f64; 2],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<([f32; 2], [f64; 2])> {
        Ok((a, b))
    }

    async fn list_roundtrip(
        &self,
        _cx: Ctx,
        a: [u8; 12],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<[u8; 12]> {
        Ok(a)
    }

    async fn list_result(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<[u8; 8]> {
        Ok([b'0', b'1', b'A', b'B', b'a', b'b', 128, 255])
    }

    async fn nested_roundtrip(
        &self,
        _cx: Ctx,
        a: [[u32; 2]; 2],
        b: [[i32; 2]; 2],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<([[u32; 2]; 2], [[i32; 2]; 2])> {
        Ok((a, b))
    }

    async fn large_roundtrip(
        &self,
        _cx: Ctx,
        a: [[u32; 2]; 2],
        b: [[i32; 4]; 4],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<([[u32; 2]; 2], [[i32; 4]; 4])> {
        Ok((a, b))
    }

    async fn nightmare_on_cpp(
        &self,
        _cx: Ctx,
        a: [Nested; 2],
    ) -> ::wit_bindgen_wrpc::anyhow::Result<[Nested; 2]> {
        Ok(a)
    }
}
