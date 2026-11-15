
#[derive(Clone)]
pub struct Component;

impl crate::server::exports::a::b::x::Handler<crate::Client> for Component {
    async fn f1(&self, cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let (tx, rx) = cx.connect().await?;
        crate::server::a::b::x::f1(tx, rx).await
    }
}

impl crate::server::exports::other::c::x::Handler<crate::Client> for Component {
    async fn f2(&self, cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let (tx, rx) = cx.connect().await?;
        crate::server::other::c::x::f2(tx, rx).await
    }
}
