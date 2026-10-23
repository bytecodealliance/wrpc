use core::pin::Pin;
use core::task::{Context, Poll};

use futures::future::BoxFuture;
use wasmtime::StoreContextMut;
use wasmtime::component::{FutureProducer, Resource};

use crate::rpc::Error;
use crate::{WrpcView, WrpcViewExt as _};

mod context;
mod error;
mod invoker;
mod transport;

struct ResultProducer(BoxFuture<'static, Result<(), Error>>);

impl<D: WrpcView + 'static> FutureProducer<D> for ResultProducer {
    type Item = Result<(), Resource<Error>>;

    fn poll_produce(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let res = match self.0.as_mut().poll(cx) {
            Poll::Ready(res) => res,
            Poll::Pending if finish => return Poll::Ready(Ok(None)),
            Poll::Pending => return Poll::Pending,
        };
        match res {
            Ok(()) => Poll::Ready(Ok(Some(Ok(())))),
            Err(err) => {
                let err = store.data_mut().push_error(err)?;
                Poll::Ready(Ok(Some(Err(err))))
            }
        }
    }
}
