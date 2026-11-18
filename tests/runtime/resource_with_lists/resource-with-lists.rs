use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::test::resource_with_lists::test::{
    Handler, HandlerThing, Thing as ThingExport,
};
use crate::server::test::resource_with_lists::test::Thing;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {}

impl HandlerThing for Component {
    async fn new(
        &self,
        l: Bytes,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ThingExport>> {
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing".as_bytes());
        let (tx, rx) = self.0.connect().await?;
        let result = Thing::new(tx, rx, &result.into()).await?;
        Ok(ResourceOwn::from(Bytes::from(result)))
    }

    async fn foo(
        &self,
        self_: ResourceBorrow<ThingExport>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = self.0.connect().await?;
        let list = Thing::foo(tx, rx, &val).await?;
        let mut list = list.to_vec();
        list.extend_from_slice(" Thing.foo".as_bytes());
        Ok(list.into())
    }

    async fn bar(
        &self,
        self_: ResourceBorrow<ThingExport>,
        l: Bytes,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing.bar".as_bytes());
        let (tx, rx) = self.0.connect().await?;
        Thing::bar(tx, rx, &val, &result.into()).await
    }

    async fn baz(&self, l: Bytes) -> ::wit_bindgen_wrpc::anyhow::Result<Bytes> {
        let mut result = l.to_vec();
        result.extend_from_slice(" Thing.baz".as_bytes());
        let (tx, rx) = self.0.connect().await?;
        let list2 = Thing::baz(tx, rx, &result.into()).await?;
        let mut list2 = list2.to_vec();
        list2.extend_from_slice(" Thing.baz again".as_bytes());
        Ok(list2.into())
    }
}
