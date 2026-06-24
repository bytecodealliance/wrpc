//@ args = [
//@   '-dHash',
//@   '-dClone',
//@   '-dstd::cmp::PartialEq',
//@   '-dcore::cmp::Eq',
//@   '--additional-derive-ignore=ignoreme',
//@ ]

use crate::server::exports::my::inline::blag::{Handler as HandlerBlag, HandlerInputStream};
use crate::server::exports::my::inline::blah::{Foo, Handler as HandlerBlah, Ignoreme};
use std::collections::{hash_map::RandomState, HashSet};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> HandlerBlah<Ctx> for Component {
    async fn bar(&self, _cx: Ctx, cool: Foo) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let _blah: HashSet<Foo, RandomState> = HashSet::from_iter([
            Foo {
                field1: "hello".to_string(),
                field2: vec![1, 2, 3],
            },
            cool,
        ]);
        Ok(())
    }

    async fn barry(&self, _cx: Ctx, _warm: Ignoreme) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl<Ctx: Send> HandlerBlag<Ctx> for Component {}

impl<Ctx: Send> HandlerInputStream<Ctx> for Component {
    async fn read(
        &self,
        _cx: Ctx,
        _stream: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<
            crate::server::exports::my::inline::blag::InputStream,
        >,
        _len: u64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<::wit_bindgen_wrpc::bytes::Bytes> {
        todo!()
    }
}
