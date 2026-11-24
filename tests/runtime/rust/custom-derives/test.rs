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
pub struct Component(pub crate::Client);

impl HandlerBlah for Component {
    async fn bar(&self, cool: Foo) -> ::std::io::Result<()> {
        let _blah: HashSet<Foo, RandomState> = HashSet::from_iter([
            Foo {
                field1: "hello".to_string(),
                field2: vec![1, 2, 3],
            },
            cool,
        ]);
        Ok(())
    }

    async fn barry(&self, _warm: Ignoreme) -> ::std::io::Result<()> {
        Ok(())
    }
}

impl HandlerBlag for Component {}

impl HandlerInputStream for Component {
    async fn read(
        &self,
        _stream: ::wit_bindgen_wrpc::wrpc_core::ResourceBorrow<
            crate::server::exports::my::inline::blag::InputStream,
        >,
        _len: u64,
    ) -> ::std::io::Result<::wit_bindgen_wrpc::bytes::Bytes> {
        todo!()
    }
}
