use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{Connect, ResourceBorrow, ResourceOwn};

use crate::server::exports::exports::{
    Float as FloatExport, Handler as HandlerExports, HandlerFloat,
};
use crate::server::imports::Float as ImportFloat1;
use crate::server::test::resource_floats::test::Float as ImportFloat2;

#[derive(Clone)]
pub struct Component;

impl<C: Connect<Context = ()>> Handler<C> for Component {
    async fn add(
        &self,
        cx: C,
        a: ResourceBorrow<Float>,
        b: ResourceBorrow<Float>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Float>> {
        let a = ImportFloat2::get(&cx, (), &a).await?;
        let b = ImportFloat2::get(&cx, (), &b).await?;
        ImportFloat2::new(&cx, (), a + b + 5.0).await
    }
}

impl<C: Connect<Context = ()>> HandlerExports<C> for Component {}

impl<C: Connect<Context = ()>> HandlerFloat<C> for Component {
    async fn new(
        &self,
        cx: C,
        v: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<FloatExport>> {
        let val = ImportFloat1::new(&cx, (), v + 1.0).await?;
        Ok(ResourceOwn::from(Bytes::from(val)))
    }

    async fn get(
        &self,
        cx: C,
        self_: ResourceBorrow<FloatExport>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<f64> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let v = ImportFloat1::get(&cx, (), &val).await?;
        Ok(v + 3.0)
    }

    async fn add(
        &self,
        cx: C,
        a: ResourceOwn<FloatExport>,
        b: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<FloatExport>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let a = ImportFloat1::add(&cx, (), &a, b).await?;
        let v = ImportFloat1::get(&cx, (), &a.as_borrow()).await?;
        self.new(cx, v + 5.0).await
    }
}
