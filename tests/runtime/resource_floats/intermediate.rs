use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

use crate::server::exports::exports::{
    Float as FloatExport, Handler as HandlerExports, HandlerFloat,
};
use crate::server::imports::Float as ImportFloat1;
use crate::server::test::resource_floats::test::Float as ImportFloat2;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn add(
        &self,
        a: ResourceBorrow<Float>,
        b: ResourceBorrow<Float>,
    ) -> ::std::io::Result<ResourceOwn<Float>> {
        let mut clt = self.0.connect().await?;
        let a = ImportFloat2::get(&mut clt, &a).await.map_err(std::io::Error::other)?;
        let b = ImportFloat2::get(&mut clt, &b).await.map_err(std::io::Error::other)?;
        ImportFloat2::new(&mut clt, a + b + 5.0).await.map_err(std::io::Error::other)
    }
}

impl HandlerExports for Component {}

impl HandlerFloat for Component {
    async fn new(
        &self,
        v: f64,
    ) -> ::std::io::Result<ResourceOwn<FloatExport>> {
        let clt = self.0.connect().await?;
        let val = ImportFloat1::new(clt, v + 1.0).await.map_err(std::io::Error::other)?;
        Ok(ResourceOwn::from(Bytes::from(val)))
    }

    async fn get(
        &self,
        self_: ResourceBorrow<FloatExport>,
    ) -> ::std::io::Result<f64> {
        let val = ResourceBorrow::from(Bytes::from(self_));
        let clt = self.0.connect().await?;
        let v = ImportFloat1::get(clt, &val).await.map_err(std::io::Error::other)?;
        Ok(v + 3.0)
    }

    async fn add(
        &self,
        a: ResourceOwn<FloatExport>,
        b: f64,
    ) -> ::std::io::Result<ResourceOwn<FloatExport>> {
        let a = ResourceOwn::from(Bytes::from(a));
        let mut clt = self.0.connect().await?;
        let a = ImportFloat1::add(&mut clt, &a, b).await.map_err(std::io::Error::other)?;
        let v = ImportFloat1::get(&mut clt, &a.as_borrow()).await.map_err(std::io::Error::other)?;
        self.new(v + 5.0).await
    }
}
