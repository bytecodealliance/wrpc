//@ [lang]
//@ externs = ['rust_xcrate_test.rs']

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};
use rust_xcrate_test::a::test::xcrate::a_imports;
use rust_xcrate_test::b::test::xcrate::b_imports;

use crate::server::exports::test::xcrate::b_exports;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl b_exports::Handler for Component {
    async fn b(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let mut clt = self.0.connect().await?;
        a_imports::f(&mut clt).await?;
        b_imports::b(&mut clt).await?;

        let x = a_imports::X::new(&mut clt).await?;
        a_imports::X::foo(&mut clt, &x.as_borrow()).await?;
        drop(x);

        let x = b_imports::X::new(&mut clt).await?;
        b_imports::X::foo(&mut clt, &x.as_borrow()).await?;
        drop(x);
        Ok(())
    }
}

impl b_exports::HandlerX for Component {
    async fn new(&self) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<b_exports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _self_: ResourceBorrow<b_exports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
