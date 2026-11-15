use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use futures::task::noop_waker_ref;

use crate::server::exports::my::test::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn cancel_before_read(
        &self,
        _cx: Ctx,
        x: Pin<Box<dyn Future<Output = u32> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        drop(x);
        Ok(())
    }

    async fn cancel_after_read(
        &self,
        _cx: Ctx,
        mut x: Pin<Box<dyn Future<Output = u32> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let _ = x.as_mut().poll(&mut Context::from_waker(noop_waker_ref()));
        drop(x);
        Ok(())
    }

    async fn start_read_then_cancel(
        &self,
        _cx: Ctx,
        mut data: Pin<Box<dyn Future<Output = u32> + Send>>,
        signal: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        let res = data
            .as_mut()
            .poll(&mut Context::from_waker(noop_waker_ref()));
        signal.await;
        let v = match res {
            Poll::Ready(v) => v,
            Poll::Pending => data.await,
        };
        assert_eq!(v, 4);
        Ok(())
    }
}
