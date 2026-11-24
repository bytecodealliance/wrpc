use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_import_and_export::test::{
    Handler as HandlerTest, HandlerThing, Thing as ExportThing,
};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn toplevel_export(
        &self,
        input: ResourceOwn<Thing>,
    ) -> ::std::io::Result<ResourceOwn<Thing>> {
        let clt = self.0.connect().await?;
        toplevel_import(clt, &input).await.map_err(std::io::Error::other)
    }
}

impl HandlerTest for Component {}

impl HandlerThing for Component {
    async fn new(
        &self,
        v: u32,
    ) -> ::std::io::Result<ResourceOwn<ExportThing>> {
        let clt = self.0.connect().await?;
        let thing = Thing::new(clt, v + 1).await.map_err(std::io::Error::other)?;
        Ok(ResourceOwn::from(Bytes::from(thing)))
    }

    async fn foo(
        &self,
        self_: ResourceBorrow<ExportThing>,
    ) -> ::std::io::Result<u32> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let clt = self.0.connect().await?;
        let v = Thing::foo(clt, &thing).await.map_err(std::io::Error::other)?;
        Ok(v + 2)
    }

    async fn bar(
        &self,
        self_: ResourceBorrow<ExportThing>,
        v: u32,
    ) -> ::std::io::Result<()> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let clt = self.0.connect().await?;
        Thing::bar(clt, &thing, v + 3).await.map_err(std::io::Error::other)
    }

    async fn baz(
        &self,
        a: ResourceOwn<ExportThing>,
        b: ResourceOwn<ExportThing>,
    ) -> ::std::io::Result<ResourceOwn<ExportThing>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let b = ResourceOwn::from(Bytes::from(b));
        let mut clt = self.0.connect().await?;
        let thing = Thing::baz(&mut clt, &a, &b).await.map_err(std::io::Error::other)?;
        let result = Thing::foo(clt, &thing.as_borrow()).await.map_err(std::io::Error::other)?;
        HandlerThing::new(self, result + 4).await
    }
}
