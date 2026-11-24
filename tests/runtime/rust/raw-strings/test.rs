use crate::server::exports::cat::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn foo(&self, x: String) -> ::std::io::Result<()> {
        assert_eq!(x, "hello");
        Ok(())
    }

    async fn bar(&self) -> ::std::io::Result<String> {
        Ok("world".into())
    }
}
