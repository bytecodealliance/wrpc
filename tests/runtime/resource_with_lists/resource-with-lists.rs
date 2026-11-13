use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{Connect, ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_with_lists::test::{
    Handler, HandlerThing, Thing as ThingExport,
};
use crate::server::test::resource_with_lists::test::Thing;

#[derive(Clone)]
pub struct Component;

impl<C: Connect<Context = ()>> Handler<C> for Component {}

impl<C: Connect<Context = ()>> HandlerThing<C> for Component {
    async fn new(
        &self,
        cx: C,
        l: Bytes,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ThingExport>> {
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing".as_bytes());
        let result = Thing::new(&cx, (), &result.into()).await?;
        Ok(ResourceOwn::from(Bytes::from(result)))
    }

    async fn foo(
        &self,
        cx: C,
        self_: ResourceBorrow<ThingExport>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let list = Thing::foo(&cx, (), &val).await?;
        let mut list = list.to_vec();
        list.extend_from_slice(" Thing.foo".as_bytes());
        Ok(list.into())
    }

    async fn bar(
        &self,
        cx: C,
        self_: ResourceBorrow<ThingExport>,
        l: Bytes,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing.bar".as_bytes());
        Thing::bar(&cx, (), &val, &result.into()).await
    }

    async fn baz(&self, cx: C, l: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing.baz".as_bytes());
        let list2 = Thing::baz(&cx, (), &result.into()).await?;
        let mut list2 = list2.to_vec();
        list2.extend_from_slice(" Thing.baz again".as_bytes());
        Ok(list2.into())
    }
}
