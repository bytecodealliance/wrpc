use crate::server::exports::test::resource_aggregates::to_test::{
    Handler, HandlerThing, Thing, R1, R2, R3, T1, T2, V1, V2,
};

#[derive(Clone)]
pub struct Component;

fn val(repr: &[u8]) -> u32 {
    u32::from_le_bytes(repr.try_into().unwrap())
}

impl<Ctx: Send> Handler<Ctx> for Component {
    #[allow(clippy::too_many_arguments)]
    async fn foo(
        &self,
        _cx: Ctx,
        r1: R1,
        r2: R2,
        r3: R3,
        t1: T1,
        t2: T2,
        v1: V1,
        v2: V2,
        l1: Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>>,
        l2: Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Thing>>,
        o1: Option<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>>,
        o2: Option<::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Thing>>,
        result1: Result<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>, ()>,
        result2: Result<::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Thing>, ()>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let mut sum = 0;
        sum += val(r1.thing.as_ref());
        sum += val(r2.thing.as_ref());
        sum += val(r3.thing1.as_ref());
        sum += val(r3.thing2.as_ref());
        sum += val(t1.0.as_ref());
        sum += val(t1.1.thing.as_ref());
        sum += val(t2.0.as_ref());
        match v1 {
            V1::Thing(v) => sum += val(v.as_ref()),
        }
        match v2 {
            V2::Thing(v) => sum += val(v.as_ref()),
        }
        for v in &l1 {
            sum += val(v.as_ref());
        }
        for v in &l2 {
            sum += val(v.as_ref());
        }
        if let Some(v) = &o1 {
            sum += val(v.as_ref());
        }
        if let Some(v) = &o2 {
            sum += val(v.as_ref());
        }
        if let Ok(v) = &result1 {
            sum += val(v.as_ref());
        }
        if let Ok(v) = &result2 {
            sum += val(v.as_ref());
        }
        Ok(sum + 3)
    }
}

impl<Ctx: Send> HandlerThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        v: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>,
    > {
        let val = v + 1;
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(&val.to_le_bytes()),
        ))
    }
}
