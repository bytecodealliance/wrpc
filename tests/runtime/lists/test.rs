use ::wit_bindgen_wrpc::bytes::Bytes;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::test::lists::to_test::Handler<Ctx> for Component {
    async fn allocated_bytes(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(0)
    }

    async fn empty_list_param(&self, _cx: Ctx, a: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert!(a.is_empty());
        Ok(())
    }

    async fn empty_string_param(
        &self,
        _cx: Ctx,
        a: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert!(a.is_empty());
        Ok(())
    }

    async fn empty_list_result(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }

    async fn empty_string_result(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(String::new())
    }

    async fn list_param(&self, _cx: Ctx, list: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(list, &[1, 2, 3, 4][..]);
        Ok(())
    }

    async fn list_param2(&self, _cx: Ctx, ptr: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ptr, "foo");
        Ok(())
    }

    async fn list_param3(
        &self,
        _cx: Ctx,
        ptr: Vec<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ptr.len(), 3);
        assert_eq!(ptr[0], "foo");
        assert_eq!(ptr[1], "bar");
        assert_eq!(ptr[2], "baz");
        Ok(())
    }

    async fn list_param4(
        &self,
        _cx: Ctx,
        ptr: Vec<Vec<String>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ptr.len(), 2);
        assert_eq!(ptr[0][0], "foo");
        assert_eq!(ptr[0][1], "bar");
        assert_eq!(ptr[1][0], "baz");
        Ok(())
    }

    async fn list_param5(
        &self,
        _cx: Ctx,
        ptr: Vec<(u8, u32, u8)>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ptr, [(1, 2, 3), (4, 5, 6)]);
        Ok(())
    }

    async fn list_param_large(
        &self,
        _cx: Ctx,
        ptr: Vec<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ptr.len(), 1000);
        Ok(())
    }

    async fn list_result(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(Bytes::from_static(&[1, 2, 3, 4, 5]))
    }

    async fn list_result2(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("hello!".to_string())
    }

    async fn list_result3(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Vec<String>> {
        Ok(vec!["hello,".to_string(), "world!".to_string()])
    }

    async fn list_roundtrip(&self, _cx: Ctx, x: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(x.clone())
    }

    async fn string_roundtrip(
        &self,
        _cx: Ctx,
        x: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(x.clone())
    }

    async fn list_minmax8(
        &self,
        _cx: Ctx,
        a: Bytes,
        b: Vec<i8>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Bytes, Vec<i8>)> {
        Ok((a, b))
    }

    async fn list_minmax16(
        &self,
        _cx: Ctx,
        a: Vec<u16>,
        b: Vec<i16>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Vec<u16>, Vec<i16>)> {
        Ok((a, b))
    }

    async fn list_minmax32(
        &self,
        _cx: Ctx,
        a: Vec<u32>,
        b: Vec<i32>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Vec<u32>, Vec<i32>)> {
        Ok((a, b))
    }

    async fn list_minmax64(
        &self,
        _cx: Ctx,
        a: Vec<u64>,
        b: Vec<i64>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Vec<u64>, Vec<i64>)> {
        Ok((a, b))
    }

    async fn list_minmax_float(
        &self,
        _cx: Ctx,
        a: Vec<f32>,
        b: Vec<f64>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(Vec<f32>, Vec<f64>)> {
        Ok((a, b))
    }

    async fn wasi_http_headers_roundtrip(
        &self,
        _cx: Ctx,
        headers: Vec<(String, Bytes)>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Vec<(String, Bytes)>> {
        Ok(headers)
    }
}
