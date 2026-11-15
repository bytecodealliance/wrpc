use std::sync::Mutex;

use ::wit_bindgen_wrpc::bytes::Bytes;
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

use crate::server::exports::exports::{
    Handler, HandlerKebabCase, HandlerX, HandlerZ, KebabCase, X, Z,
};

static XS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn x_index(x: &[u8]) -> usize {
    usize::try_from(u32::from_le_bytes(x.try_into().unwrap())).unwrap()
}

fn z_val(z: &[u8]) -> i32 {
    i32::from_le_bytes(z.try_into().unwrap())
}

#[derive(Clone)]
pub struct Component;

impl Handler<crate::Client> for Component {
    async fn add(
        &self,
        _cx: crate::Client,
        a: ResourceBorrow<Z>,
        b: ResourceBorrow<Z>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Z>> {
        let val = z_val(a.as_ref()) + z_val(b.as_ref());
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &val.to_le_bytes(),
        )))
    }

    async fn consume(&self, _cx: crate::Client, x: ResourceOwn<X>) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        drop(x);
        Ok(())
    }

    async fn test_imports(&self, cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<Result<(), String>> {
        use crate::server::imports::*;

        let (tx, rx) = cx.connect().await?;
        let y = Y::new(tx, rx, 10).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y.as_borrow()).await?;
        assert_eq!(v, 10);
        let (tx, rx) = cx.connect().await?;
        Y::set_a(tx, rx, &y.as_borrow(), 20).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y.as_borrow()).await?;
        assert_eq!(v, 20);
        let (tx, rx) = cx.connect().await?;
        let y2 = Y::add(tx, rx, &y, 20).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y2.as_borrow()).await?;
        assert_eq!(v, 40);

        // test multiple instances
        let (tx, rx) = cx.connect().await?;
        let y1 = Y::new(tx, rx, 1).await?;
        let (tx, rx) = cx.connect().await?;
        let y2 = Y::new(tx, rx, 2).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y1.as_borrow()).await?;
        assert_eq!(v, 1);
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y2.as_borrow()).await?;
        assert_eq!(v, 2);
        let (tx, rx) = cx.connect().await?;
        Y::set_a(tx, rx, &y1.as_borrow(), 10).await?;
        let (tx, rx) = cx.connect().await?;
        Y::set_a(tx, rx, &y2.as_borrow(), 20).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y1.as_borrow()).await?;
        assert_eq!(v, 10);
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y2.as_borrow()).await?;
        assert_eq!(v, 20);
        let (tx, rx) = cx.connect().await?;
        let y3 = Y::add(tx, rx, &y1, 20).await?;
        let (tx, rx) = cx.connect().await?;
        let y4 = Y::add(tx, rx, &y2, 30).await?;
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y3.as_borrow()).await?;
        assert_eq!(v, 30);
        let (tx, rx) = cx.connect().await?;
        let v = Y::get_a(tx, rx, &y4.as_borrow()).await?;
        assert_eq!(v, 50);
        Ok(Ok(()))
    }
}

impl HandlerX<crate::Client> for Component {
    async fn new(&self, _cx: crate::Client, a: i32) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<X>> {
        let mut xs = XS.lock().unwrap();
        let idx = u32::try_from(xs.len())?;
        xs.push(a);
        Ok(ResourceOwn::from(Bytes::copy_from_slice(
            &idx.to_le_bytes(),
        )))
    }

    async fn get_a(
        &self,
        _cx: crate::Client,
        self_: ResourceBorrow<X>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<i32> {
        Ok(XS.lock().unwrap()[x_index(self_.as_ref())])
    }

    async fn set_a(
        &self,
        _cx: crate::Client,
        self_: ResourceBorrow<X>,
        a: i32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        XS.lock().unwrap()[x_index(self_.as_ref())] = a;
        Ok(())
    }

    async fn add(
        &self,
        _cx: crate::Client,
        x: ResourceOwn<X>,
        a: i32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<X>> {
        XS.lock().unwrap()[x_index(x.as_ref())] += a;
        Ok(x)
    }
}

impl HandlerZ<crate::Client> for Component {
    async fn new(&self, _cx: crate::Client, a: i32) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<Z>> {
        Ok(ResourceOwn::from(Bytes::copy_from_slice(&a.to_le_bytes())))
    }

    async fn get_a(
        &self,
        _cx: crate::Client,
        self_: ResourceBorrow<Z>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<i32> {
        Ok(z_val(self_.as_ref()))
    }

    async fn num_dropped(&self, _cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(1)
    }
}

impl HandlerKebabCase<crate::Client> for Component {
    async fn new(
        &self,
        _cx: crate::Client,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ResourceOwn<KebabCase>> {
        Ok(ResourceOwn::from(Bytes::copy_from_slice(&a.to_le_bytes())))
    }

    async fn get_a(
        &self,
        _cx: crate::Client,
        self_: ResourceBorrow<KebabCase>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        let bytes: &[u8] = self_.as_ref();
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    async fn take_owned(
        &self,
        cx: crate::Client,
        k: ResourceOwn<KebabCase>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        HandlerKebabCase::get_a(self, cx, k.into()).await
    }
}
