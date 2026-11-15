//@ [lang]
//@ externs = ['rust_xcrate_test.rs']

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};
use rust_xcrate_test::a::test::xcrate::a_imports;
use rust_xcrate_test::b::test::xcrate::b_imports;

use crate::server::exports::test::xcrate::b_exports;

#[derive(Clone)]
pub struct Component;

impl b_exports::Handler<crate::Client> for Component {
    async fn b(&self, cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let (tx, rx) = cx.connect().await?;
        a_imports::f(tx, rx).await?;
        let (tx, rx) = cx.connect().await?;
        b_imports::b(tx, rx).await?;

        let (tx, rx) = cx.connect().await?;
        let x = a_imports::X::new(tx, rx).await?;
        let (tx, rx) = cx.connect().await?;
        a_imports::X::foo(tx, rx, &x.as_borrow()).await?;
        drop(x);

        let (tx, rx) = cx.connect().await?;
        let x = b_imports::X::new(tx, rx).await?;
        let (tx, rx) = cx.connect().await?;
        b_imports::X::foo(tx, rx, &x.as_borrow()).await?;
        drop(x);
        Ok(())
    }
}

impl b_exports::HandlerX<crate::Client> for Component {
    async fn new(&self, _cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<b_exports::X>> {
        Ok(ResourceOwn::from(Bytes::default()))
    }

    async fn foo(
        &self,
        _cx: crate::Client,
        _self_: ResourceBorrow<b_exports::X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
