#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::test::exports::a::b::x::Handler for Component {
    async fn f1(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}

impl crate::test::exports::other::c::x::Handler for Component {
    async fn f2(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
