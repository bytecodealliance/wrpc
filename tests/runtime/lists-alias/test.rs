use ::wit_bindgen_wrpc::bytes::Bytes;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::cat::Handler for Component {
    async fn foo(&self, x: Bytes) -> ::std::io::Result<()> {
        assert_eq!(x, &b"hello"[..]);
        Ok(())
    }

    async fn bar(&self) -> ::std::io::Result<Bytes> {
        Ok(Bytes::from_static(b"world"))
    }
}
