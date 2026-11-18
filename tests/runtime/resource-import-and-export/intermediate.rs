use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_import_and_export::test::{
    Handler as HandlerTest, HandlerThing, Thing as ExportThing,
};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn toplevel_export(
        &self,
        input: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        let (tx, rx) = self.0.connect().await?;
        toplevel_import(tx, rx, &input).await
    }
}

impl HandlerTest for Component {}

impl HandlerThing for Component {
    async fn new(
        &self,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let (tx, rx) = self.0.connect().await?;
        let thing = Thing::new(tx, rx, v + 1).await?;
        Ok(ResourceOwn::from(Bytes::from(thing)))
    }

    async fn foo(
        &self,
        self_: ResourceBorrow<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = self.0.connect().await?;
        let v = Thing::foo(tx, rx, &thing).await?;
        Ok(v + 2)
    }

    async fn bar(
        &self,
        self_: ResourceBorrow<ExportThing>,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = self.0.connect().await?;
        Thing::bar(tx, rx, &thing, v + 3).await
    }

    async fn baz(
        &self,
        a: ResourceOwn<ExportThing>,
        b: ResourceOwn<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let b = ResourceOwn::from(Bytes::from(b));
        let (mut tx, mut rx) = self.0.connect().await?;
        let thing = Thing::baz(&mut tx, &mut rx, &a, &b).await?;
        let result = Thing::foo(tx, rx, &thing.as_borrow()).await?;
        HandlerThing::new(self, result + 4).await
    }
}
