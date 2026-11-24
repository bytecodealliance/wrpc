#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::cat::Handler for Component {
    async fn foo(&self, x: String) -> ::std::io::Result<()> {
        assert_eq!(x, "hello");
        Ok(())
    }

    async fn bar(&self) -> ::std::io::Result<String> {
        Ok("world".into())
    }
}
