use ::wit_bindgen_wrpc::bytes::Bytes;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::lists::to_test::Handler for Component {
    async fn allocated_bytes(&self) -> ::std::io::Result<u32> {
        Ok(0)
    }

    async fn empty_list_param(&self, a: Bytes) -> ::std::io::Result<()> {
        assert!(a.is_empty());
        Ok(())
    }

    async fn empty_string_param(
        &self,
        a: String,
    ) -> ::std::io::Result<()> {
        assert!(a.is_empty());
        Ok(())
    }

    async fn empty_list_result(&self) -> ::std::io::Result<Bytes> {
        Ok(Bytes::new())
    }

    async fn empty_string_result(&self) -> ::std::io::Result<String> {
        Ok(String::new())
    }

    async fn list_param(&self, list: Bytes) -> ::std::io::Result<()> {
        assert_eq!(list, &[1, 2, 3, 4][..]);
        Ok(())
    }

    async fn list_param2(&self, ptr: String) -> ::std::io::Result<()> {
        assert_eq!(ptr, "foo");
        Ok(())
    }

    async fn list_param3(
        &self,
        ptr: Vec<String>,
    ) -> ::std::io::Result<()> {
        assert_eq!(ptr.len(), 3);
        assert_eq!(ptr[0], "foo");
        assert_eq!(ptr[1], "bar");
        assert_eq!(ptr[2], "baz");
        Ok(())
    }

    async fn list_param4(
        &self,
        ptr: Vec<Vec<String>>,
    ) -> ::std::io::Result<()> {
        assert_eq!(ptr.len(), 2);
        assert_eq!(ptr[0][0], "foo");
        assert_eq!(ptr[0][1], "bar");
        assert_eq!(ptr[1][0], "baz");
        Ok(())
    }

    async fn list_param5(
        &self,
        ptr: Vec<(u8, u32, u8)>,
    ) -> ::std::io::Result<()> {
        assert_eq!(ptr, [(1, 2, 3), (4, 5, 6)]);
        Ok(())
    }

    async fn list_param_large(
        &self,
        ptr: Vec<String>,
    ) -> ::std::io::Result<()> {
        assert_eq!(ptr.len(), 1000);
        Ok(())
    }

    async fn list_result(&self) -> ::std::io::Result<Bytes> {
        Ok(Bytes::from_static(&[1, 2, 3, 4, 5]))
    }

    async fn list_result2(&self) -> ::std::io::Result<String> {
        Ok("hello!".to_string())
    }

    async fn list_result3(&self) -> ::std::io::Result<Vec<String>> {
        Ok(vec!["hello,".to_string(), "world!".to_string()])
    }

    async fn list_roundtrip(&self, x: Bytes) -> ::std::io::Result<Bytes> {
        Ok(x.clone())
    }

    async fn string_roundtrip(
        &self,
        x: String,
    ) -> ::std::io::Result<String> {
        Ok(x.clone())
    }

    async fn list_minmax8(
        &self,
        a: Bytes,
        b: Vec<i8>,
    ) -> ::std::io::Result<(Bytes, Vec<i8>)> {
        Ok((a, b))
    }

    async fn list_minmax16(
        &self,
        a: Vec<u16>,
        b: Vec<i16>,
    ) -> ::std::io::Result<(Vec<u16>, Vec<i16>)> {
        Ok((a, b))
    }

    async fn list_minmax32(
        &self,
        a: Vec<u32>,
        b: Vec<i32>,
    ) -> ::std::io::Result<(Vec<u32>, Vec<i32>)> {
        Ok((a, b))
    }

    async fn list_minmax64(
        &self,
        a: Vec<u64>,
        b: Vec<i64>,
    ) -> ::std::io::Result<(Vec<u64>, Vec<i64>)> {
        Ok((a, b))
    }

    async fn list_minmax_float(
        &self,
        a: Vec<f32>,
        b: Vec<f64>,
    ) -> ::std::io::Result<(Vec<f32>, Vec<f64>)> {
        Ok((a, b))
    }

    async fn wasi_http_headers_roundtrip(
        &self,
        headers: Vec<(String, Bytes)>,
    ) -> ::std::io::Result<Vec<(String, Bytes)>> {
        Ok(headers)
    }
}
