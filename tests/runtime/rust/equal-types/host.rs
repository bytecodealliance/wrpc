//@ args = '--merge-structurally-equal-types'

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::host::exports::test::equal_types::blag::{
    Handler, HandlerInputStream, InputStream, Kind1, Kind3, Kind4, TStream, Tree,
};
use crate::host::exports::test::equal_types::blah::{
    CustomResult, Handler as HHandler, Kind5, Kind6, Kind7, R1, R2,
};
use crate::host::exports::test::equal_types::resources;

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
        Ok(x)
    }

    async fn g(&self, x: Kind3) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        Ok(Kind4 { a: x.a })
    }

    async fn h(&self, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<Tree> {
        Ok(x.tree)
    }
}

impl HHandler for Component {
    async fn f(&self, x: Kind6) -> ::wit_bindgen_wrpc::anyhow::Result<Kind5> {
        Ok(match x {
            Kind6::A => Kind1::A,
            Kind6::B(x) => Kind5::B(x),
            Kind6::C => Kind1::C,
        })
    }

    async fn g(&self, x: Kind7) -> ::wit_bindgen_wrpc::anyhow::Result<Kind4> {
        Ok(Kind4 {
            a: ResourceOwn::from(Bytes::copy_from_slice(x.a.as_ref())),
        })
    }

    async fn h(&self, x: TStream) -> ::wit_bindgen_wrpc::anyhow::Result<CustomResult> {
        Ok(CustomResult::Ok(x.tree))
    }

    async fn alias_type(&self, x: R2) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        Ok(x)
    }
}

impl resources::Handler for Component {
    async fn alias_own(
        &self,
        x: resources::T2,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<resources::T1> {
        Ok(x)
    }

    async fn alias_aggregate(
        &self,
        x: Option<resources::T2>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<resources::T1>> {
        Ok(x)
    }
}

impl resources::HandlerR1 for Component {}
