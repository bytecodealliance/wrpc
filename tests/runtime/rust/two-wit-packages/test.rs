#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::test::exports::a::b::x::Handler for Component {
    async fn f1(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl crate::test::exports::other::c::x::Handler for Component {
    async fn f2(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
