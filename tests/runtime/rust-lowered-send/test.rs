use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn one_argument(&self, x: String) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(&x, "hello");
        Ok(())
    }
}
