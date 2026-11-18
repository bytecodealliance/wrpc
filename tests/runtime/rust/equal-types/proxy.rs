//@ args = '--merge-structurally-equal-types'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow};

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
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(Bytes::default())
    }
}

impl Handler for Component {
    async fn f(&self, x: Kind1) -> ::wit_bindgen_wrpc::anyhow::Result<Kind1> {
        let (tx, rx) = self.0.connect().await?;
        blag::f(tx, rx, &x).await
    }

    async fn g(&self, _x: Kind3) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        todo!()
    }

    async fn h(&self, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<Tree> {
        let x = blag::TStream {
            tree: x.tree,
            stream: None,
        };
        let (tx, rx) = self.0.connect().await?;
        blag::h(tx, rx, &x).await
    }
}

use crate::server::exports::test::equal_types::blah::{
    CustomResult, Handler as HHandler, Kind5, Kind6, Kind7, R1, R2,
};
use crate::server::test::equal_types::blah;

impl HHandler for Component {
    async fn f(&self, x: Kind6) -> ::wit_bindgen_wrpc::anyhow::Result<Kind5> {
        let (tx, rx) = self.0.connect().await?;
        blah::f(tx, rx, &x).await
    }

    async fn g(&self, _x: Kind7) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        todo!()
    }

    async fn h(&self, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<CustomResult> {
        let x = blah::TStream {
            tree: x.tree,
            stream: None,
        };
        let (tx, rx) = self.0.connect().await?;
        blah::h(tx, rx, &x).await
    }

    async fn alias_type(&self, x: R2) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        Ok(x)
    }
}
