//@ args = '--merge-structurally-equal-types'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{Connect, ResourceBorrow};

use crate::server::exports::test::equal_types::blag::{
    Handler, HandlerInputStream, InputStream, Kind1, Kind3, Kind4, TStream, Tree,
};
use crate::server::test::equal_types::blag;

#[derive(Clone)]
pub struct Component;

impl<C: Connect<Context = ()>> HandlerInputStream<C> for Component {
    async fn read(
        &self,
        _cx: C,
        _self_: ResourceBorrow<InputStream>,
        _len: u64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        Ok(Bytes::default())
    }
}

impl<C: Connect<Context = ()>> Handler<C> for Component {
    async fn f(&self, cx: C, x: Kind1) -> ::wit_bindgen_wrpc::anyhow::Result<Kind1> {
        blag::f(&cx, (), &x).await
    }

    async fn g(&self, _cx: C, _x: Kind3) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        todo!()
    }

    async fn h(&self, cx: C, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<Tree> {
        let x = blag::TStream {
            tree: x.tree,
            stream: None,
        };
        blag::h(&cx, (), &x).await
    }
}

use crate::server::exports::test::equal_types::blah::{
    CustomResult, Handler as HHandler, Kind5, Kind6, Kind7, R1, R2,
};
use crate::server::test::equal_types::blah;

impl<C: Connect<Context = ()>> HHandler<C> for Component {
    async fn f(&self, cx: C, x: Kind6) -> ::wit_bindgen_wrpc::anyhow::Result<Kind5> {
        blah::f(&cx, (), &x).await
    }

    async fn g(&self, _cx: C, _x: Kind7) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        todo!()
    }

    async fn h(&self, cx: C, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<CustomResult> {
        let x = blah::TStream {
            tree: x.tree,
            stream: None,
        };
        blah::h(&cx, (), &x).await
    }

    async fn alias_type(&self, _cx: C, x: R2) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        Ok(x)
    }
}
