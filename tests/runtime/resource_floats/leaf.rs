use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::leaf::exports::imports::{
    Float as ImportFloat1, Handler as Guest, HandlerFloat as GuestFloat,
};
use crate::leaf::exports::test::resource_floats::test::{
    Float as Float2, Handler as Guest2, HandlerFloat as GuestFloat2,
};

fn my_float<T: ?Sized>(v: f64) -> ResourceOwn<T> {
    ResourceOwn::from(Bytes::copy_from_slice(&v.to_le_bytes()))
}

fn val(repr: &[u8]) -> f64 {
    f64::from_le_bytes(repr.try_into().unwrap())
}

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Guest<Ctx> for Component {}

impl<Ctx: Send> GuestFloat<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        v: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ImportFloat1>> {
        Ok(my_float(v + 2.0))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<ImportFloat1>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<f64> {
        Ok(val(self_.as_ref()) + 4.0)
    }

    async fn add(
        &self,
        cx: Ctx,
        a: ResourceOwn<ImportFloat1>,
        b: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<ImportFloat1>> {
        GuestFloat::new(self, cx, val(a.as_ref()) + b + 6.0).await
    }
}

impl<Ctx: Send> Guest2<Ctx> for Component {}

impl<Ctx: Send> GuestFloat2<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        v: f64,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Float2>> {
        Ok(my_float(v + 1.0))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ResourceBorrow<Float2>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<f64> {
        Ok(val(self_.as_ref()) + 3.0)
    }
}
