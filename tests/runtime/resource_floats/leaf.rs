use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_core::{ResourceBorrow, ResourceOwn};

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
pub struct Component(pub crate::Client);

impl Guest for Component {}

impl GuestFloat for Component {
    async fn new(
        &self,
        v: f64,
    ) -> ::std::io::Result<ResourceOwn<ImportFloat1>> {
        Ok(my_float(v + 2.0))
    }

    async fn get(
        &self,
        self_: ResourceBorrow<ImportFloat1>,
    ) -> ::std::io::Result<f64> {
        Ok(val(self_.as_ref()) + 4.0)
    }

    async fn add(
        &self,
        a: ResourceOwn<ImportFloat1>,
        b: f64,
    ) -> ::std::io::Result<ResourceOwn<ImportFloat1>> {
        GuestFloat::new(self, val(a.as_ref()) + b + 6.0).await
    }
}

impl Guest2 for Component {}

impl GuestFloat2 for Component {
    async fn new(
        &self,
        v: f64,
    ) -> ::std::io::Result<ResourceOwn<Float2>> {
        Ok(my_float(v + 1.0))
    }

    async fn get(
        &self,
        self_: ResourceBorrow<Float2>,
    ) -> ::std::io::Result<f64> {
        Ok(val(self_.as_ref()) + 3.0)
    }
}
