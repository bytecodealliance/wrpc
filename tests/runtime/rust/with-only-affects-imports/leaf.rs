use crate::leaf::exports::my::inline::foo::{A, Handler};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn bar(&self, a: A) -> ::std::io::Result<()> {
        assert_eq!(a.b, 2);
        Ok(())
    }
}
