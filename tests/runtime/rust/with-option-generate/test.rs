//@ args = '--generate-all'

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::foo::baz::a::Handler for Component {
    async fn x(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
