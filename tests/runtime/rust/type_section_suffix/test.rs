#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::bar::Handler for Component {
    async fn f(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}

impl crate::server::exports::foo::Handler for Component {
    async fn f(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}

impl crate::server::exports::test::suffix::imports::Handler for Component {
    async fn foo(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
