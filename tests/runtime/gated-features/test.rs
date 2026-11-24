//@ args = '--features y'

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::foo::bar::bindings::Handler for Component {
    async fn y(&self) -> ::std::io::Result<()> {
        Ok(())
    }

    async fn z(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
