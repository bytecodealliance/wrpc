use crate::server::exports::cat::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn foo(&self, x: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(x, "hello");
        Ok(())
    }

    async fn bar(&self) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("world".into())
    }
}
