use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn one_argument(&self, x: String) -> ::std::io::Result<()> {
        assert_eq!(&x, "hello");
        Ok(())
    }
}
