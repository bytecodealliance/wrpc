use ::wit_bindgen_wrpc::bytes::Bytes;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::cat::Handler<Ctx> for Component {
    async fn foo(&self, _cx: Ctx, x: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(x, &b"hello"[..]);
        Ok(())
    }

    async fn bar(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(Bytes::from_static(b"world"))
    }
}
