use crate::server::exports::a::b::the_test::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn x(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
