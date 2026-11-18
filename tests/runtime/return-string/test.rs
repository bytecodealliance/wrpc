use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn return_string(&self) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok("hello".into())
    }
}
