
#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::a::b::x::Handler for Component {
    async fn f1(&self) -> ::std::io::Result<()> {
        let clt = self.0.connect().await?;
        crate::server::a::b::x::f1(clt).await.map_err(std::io::Error::other)
    }
}

impl crate::server::exports::other::c::x::Handler for Component {
    async fn f2(&self) -> ::std::io::Result<()> {
        let clt = self.0.connect().await?;
        crate::server::other::c::x::f2(clt).await.map_err(std::io::Error::other)
    }
}
