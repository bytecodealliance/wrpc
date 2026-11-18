//@ args = '--features y'

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::foo::bar::bindings::Handler for Component {
    async fn y(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }

    async fn z(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
