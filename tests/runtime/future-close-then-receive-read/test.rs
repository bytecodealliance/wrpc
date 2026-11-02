use core::future::Future;
use core::pin::Pin;

use std::sync::Mutex;

use crate::server::exports::a::b::the_test::Handler;

static SLOT: Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send>>>> = Mutex::new(None);

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn set(
        &self,
        _cx: Ctx,
        future: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        *SLOT.lock().unwrap() = Some(future);
        Ok(())
    }

    async fn get(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Pin<Box<dyn Future<Output = ()> + Send>>> {
        Ok(SLOT.lock().unwrap().take().unwrap())
    }
}
