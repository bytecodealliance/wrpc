use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_import_and_export::test::{
    Handler as HandlerTest, HandlerThing, Thing as ExportThing,
};

#[derive(Clone)]
pub struct Component;

impl Handler<crate::Client> for Component {
    async fn toplevel_export(
        &self,
        cx: crate::Client,
        input: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        let (tx, rx) = cx.connect().await?;
        toplevel_import(tx, rx, &input).await
    }
}

impl HandlerTest<crate::Client> for Component {}

impl HandlerThing<crate::Client> for Component {
    async fn new(
        &self,
        cx: crate::Client,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let (tx, rx) = cx.connect().await?;
        let thing = Thing::new(tx, rx, v + 1).await?;
        Ok(ResourceOwn::from(Bytes::from(thing)))
    }

    async fn foo(
        &self,
        cx: crate::Client,
        self_: ResourceBorrow<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = cx.connect().await?;
        let v = Thing::foo(tx, rx, &thing).await?;
        Ok(v + 2)
    }

    async fn bar(
        &self,
        cx: crate::Client,
        self_: ResourceBorrow<ExportThing>,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = cx.connect().await?;
        Thing::bar(tx, rx, &thing, v + 3).await
    }

    async fn baz(
        &self,
        cx: crate::Client,
        a: ResourceOwn<ExportThing>,
        b: ResourceOwn<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let b = ResourceOwn::from(Bytes::from(b));
        let (tx, rx) = cx.connect().await?;
        let thing = Thing::baz(tx, rx, &a, &b).await?;
        let (tx, rx) = cx.connect().await?;
        let result = Thing::foo(tx, rx, &thing.as_borrow()).await?;
        HandlerThing::new(self, cx, result + 4).await
    }
}
