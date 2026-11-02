use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{Invoke, ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_import_and_export::test::{
    Handler as HandlerTest, HandlerThing, Thing as ExportThing,
};

#[derive(Clone)]
pub struct Component;

impl<C: Invoke<Context = ()>> Handler<C> for Component {
    async fn toplevel_export(
        &self,
        cx: C,
        input: ResourceOwn<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Thing>> {
        toplevel_import(&cx, (), &input).await
    }
}

impl<C: Invoke<Context = ()>> HandlerTest<C> for Component {}

impl<C: Invoke<Context = ()>> HandlerThing<C> for Component {
    async fn new(
        &self,
        cx: C,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let thing = Thing::new(&cx, (), v + 1).await?;
        Ok(ResourceOwn::from(Bytes::from(thing)))
    }

    async fn foo(
        &self,
        cx: C,
        self_: ResourceBorrow<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        let v = Thing::foo(&cx, (), &thing).await?;
        Ok(v + 2)
    }

    async fn bar(
        &self,
        cx: C,
        self_: ResourceBorrow<ExportThing>,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let thing = ResourceBorrow::from(Bytes::from(self_));
        Thing::bar(&cx, (), &thing, v + 3).await
    }

    async fn baz(
        &self,
        cx: C,
        a: ResourceOwn<ExportThing>,
        b: ResourceOwn<ExportThing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ExportThing>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let b = ResourceOwn::from(Bytes::from(b));
        let thing = Thing::baz(&cx, (), &a, &b).await?;
        let result = Thing::foo(&cx, (), &thing.as_borrow()).await?;
        HandlerThing::new(self, cx, result + 4).await
    }
}
