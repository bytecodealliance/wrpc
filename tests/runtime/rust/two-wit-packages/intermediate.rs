
#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::a::b::x::Handler for Component {
    async fn f1(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let (tx, rx) = self.0.connect().await?;
        crate::server::a::b::x::f1(tx, rx).await
    }
}

impl crate::server::exports::other::c::x::Handler for Component {
    async fn f2(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let (tx, rx) = self.0.connect().await?;
        crate::server::other::c::x::f2(tx, rx).await
    }
}
