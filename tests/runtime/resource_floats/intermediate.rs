use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::exports::{
    Float as FloatExport, Handler as HandlerExports, HandlerFloat,
};
use crate::server::imports::Float as ImportFloat1;
use crate::server::test::resource_floats::test::Float as ImportFloat2;

#[derive(Clone)]
pub struct Component;

impl Handler<crate::Client> for Component {
    async fn add(
        &self,
        cx: crate::Client,
        a: ResourceBorrow<Float>,
        b: ResourceBorrow<Float>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Float>> {
        let (mut tx, mut rx) = cx.connect().await?;
        let a = ImportFloat2::get(&mut tx, &mut rx, &a).await?;
        let b = ImportFloat2::get(&mut tx, &mut rx, &b).await?;
        ImportFloat2::new(&mut tx, &mut rx, a + b + 5.0).await
    }
}

impl HandlerExports<crate::Client> for Component {}

impl HandlerFloat<crate::Client> for Component {
    async fn new(
        &self,
        cx: crate::Client,
        v: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<FloatExport>> {
        let (tx, rx) = cx.connect().await?;
        let val = ImportFloat1::new(tx, rx, v + 1.0).await?;
        Ok(ResourceOwn::from(Bytes::from(val)))
    }

    async fn get(
        &self,
        cx: crate::Client,
        self_: ResourceBorrow<FloatExport>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<f64> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let (tx, rx) = cx.connect().await?;
        let v = ImportFloat1::get(tx, rx, &val).await?;
        Ok(v + 3.0)
    }

    async fn add(
        &self,
        cx: crate::Client,
        a: ResourceOwn<FloatExport>,
        b: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<FloatExport>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let (mut tx, mut rx) = cx.connect().await?;
        let a = ImportFloat1::add(&mut tx, &mut rx, &a, b).await?;
        let v = ImportFloat1::get(&mut tx, &mut rx, &a.as_borrow()).await?;
        self.new(cx, v + 5.0).await
    }
}
