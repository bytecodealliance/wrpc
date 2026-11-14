//@ [lang]
//@ externs = ['rust_xcrate_test.rs']

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{Connect, ResourceBorrow, ResourceOwn};
use rust_xcrate_test::a::test::xcrate::a_imports;
use rust_xcrate_test::b::test::xcrate::b_imports;

use crate::server::exports::test::xcrate::b_exports;

#[derive(Clone)]
pub struct Component;

impl<C: Connect<Context = ()>> b_exports::Handler<C> for Component {
    async fn b(&self, cx: C) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        a_imports::f(&cx, ()).await?;
        b_imports::b(&cx, ()).await?;

        let x = a_imports::X::new(&cx, ()).await?;
        a_imports::X::foo(&cx, (), &x.as_borrow()).await?;
        drop(x);

        let x = b_imports::X::new(&cx, ()).await?;
        b_imports::X::foo(&cx, (), &x.as_borrow()).await?;
        drop(x);
        Ok(())
    }
}

impl<C: Connect<Context = ()>> b_exports::HandlerX<C> for Component {
    async fn new(&self, _cx: C) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<b_exports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _cx: C,
        _self_: ResourceBorrow<b_exports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
