//@ args = '--merge-structurally-equal-types'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow};

use crate::server::exports::test::equal_types::blag::{
    Handler, HandlerInputStream, InputStream, Kind1, Kind3, Kind4, TStream, Tree,
};
use crate::server::test::equal_types::blag;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl HandlerInputStream for Component {
    async fn read(
        &self,
        _self_: ResourceBorrow<InputStream>,
        _len: u64,
    ) -> ::std::io::Result<Bytes> {
        Ok(Bytes::default())
    }
}

impl Handler for Component {
    async fn f(&self, x: Kind1) -> ::std::io::Result<Kind1> {
        let clt = self.0.connect().await?;
        blag::f(clt, &x).await.map_err(std::io::Error::other)
    }

    async fn g(&self, _x: Kind3) -> ::std::io::Result<Kind4> {
        todo!()
    }

    async fn h(&self, x: TStream) -> ::std::io::Result<Tree> {
        let x = blag::TStream {
            tree: x.tree,
            stream: None,
        };
        let clt = self.0.connect().await?;
        blag::h(clt, &x).await.map_err(std::io::Error::other)
    }
}

use crate::server::exports::test::equal_types::blah::{
    CustomResult, Handler as HHandler, Kind5, Kind6, Kind7, R1, R2,
};
use crate::server::test::equal_types::blah;

impl HHandler for Component {
    async fn f(&self, x: Kind6) -> ::std::io::Result<Kind5> {
        let clt = self.0.connect().await?;
        blah::f(clt, &x).await.map_err(std::io::Error::other)
    }

    async fn g(&self, _x: Kind7) -> ::std::io::Result<Kind4> {
        todo!()
    }

    async fn h(&self, x: TStream) -> ::std::io::Result<CustomResult> {
        let x = blah::TStream {
            tree: x.tree,
            stream: None,
        };
        let clt = self.0.connect().await?;
        blah::h(clt, &x).await.map_err(std::io::Error::other)
    }

    async fn alias_type(&self, x: R2) -> ::std::io::Result<R1> {
        Ok(x)
    }
}
