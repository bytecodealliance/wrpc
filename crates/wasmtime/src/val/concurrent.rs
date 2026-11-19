use core::pin::Pin;
use core::task::{Context, Poll};

use futures::stream::{self, BoxStream, Fuse};
use futures::{StreamExt as _, TryFutureExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use tracing::warn;
use wasmtime::component::{
    self, Destination, FutureConsumer, FutureReader, Lift, Lower, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult, Type, VecBuffer,
};
use wasmtime::{AsContextMut, StoreContextMut, bail, format_err};
use wrpc_transport::READ_CHUNK_SIZE;

use crate::{FutureAny, StreamAny};

fn pipe_future<P, T>(
    store: StoreContextMut<'_, T>,
    future: component::FutureAny,
) -> wasmtime::Result<FutureAny>
where
    P: Lift + Lower + Send + Sync + 'static,
{
    struct Consumer<P>(Option<oneshot::Sender<P>>);

    impl<P: Lift + Lower + Send + Sync + 'static, D> FutureConsumer<D> for Consumer<P> {
        type Item = P;

        fn poll_consume(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            store: StoreContextMut<'_, D>,
            mut source: Source<'_, P>,
            _: bool,
        ) -> Poll<wasmtime::Result<()>> {
            let mut v = None;
            source.read(store, &mut v)?;
            if let (Some(v), Some(tx)) = (v, self.0.take()) {
                _ = tx.send(v);
            }
            Poll::Ready(Ok(()))
        }
    }

    let (tx, rx) = oneshot::channel();
    let future = future.try_into_future_reader::<P>()?;
    future.pipe(store, Consumer(Some(tx)))?;
    Ok(FutureAny::new(rx.map_err(|_| {
        std::io::Error::other("future dropped without a value")
    })))
}

fn pipe_stream<P, T>(
    store: StoreContextMut<'_, T>,
    stream: component::StreamAny,
) -> wasmtime::Result<StreamAny>
where
    P: Lift + Lower + Send + Sync + 'static,
{
    struct Consumer<P: Send + 'static>(PollSender<Vec<P>>);

    impl<P: Lift + Lower + Send + Sync + 'static, D> StreamConsumer<D> for Consumer<P> {
        type Item = P;

        fn poll_consume(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut store: StoreContextMut<'_, D>,
            mut source: Source<'_, P>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            match self.0.poll_reserve(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(..)) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
            let n = source.remaining(&mut store)?;
            if n > 0 {
                let mut buf = Vec::with_capacity(n.min(READ_CHUNK_SIZE));
                source.read(&mut store, &mut buf)?;
                if self.0.send_item(buf).is_err() {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
            Poll::Ready(Ok(StreamResult::Completed))
        }
    }

    let (tx, mut rx) = mpsc::channel(1);
    let stream = stream.try_into_stream_reader::<P>()?;
    stream.pipe(store, Consumer(PollSender::new(tx)))?;
    Ok(StreamAny::new(stream::poll_fn(move |cx| rx.poll_recv(cx))))
}

fn new_future<P, T>(
    mut store: StoreContextMut<'_, T>,
    future: FutureAny,
) -> wasmtime::Result<component::Val>
where
    P: Lift + Lower + Send + Sync + 'static,
{
    let Ok(future) = future.try_into_future::<P>() else {
        bail!("`future` type mismatch")
    };
    let future = FutureReader::new(&mut store, future)?;
    let future = future.try_into_future_any(store)?;
    Ok(component::Val::Future(future))
}

struct Producer<P>(Fuse<BoxStream<'static, Vec<P>>>);

impl<P, D> StreamProducer<D> for Producer<P>
where
    P: Lift + Lower + Send + Sync + 'static,
{
    type Item = P;
    type Buffer = VecBuffer<P>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, P, VecBuffer<P>>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let n = dst.remaining(store)?;
        if n == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        match self.0.poll_next_unpin(cx) {
            Poll::Ready(Some(chunk)) => {
                dst.set_buffer(chunk.into());
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn new_stream<P, T>(
    mut store: StoreContextMut<'_, T>,
    stream: StreamAny,
) -> wasmtime::Result<component::Val>
where
    P: Lift + Lower + Send + Sync + 'static,
{
    let Ok(items) = stream.try_into_stream::<P>() else {
        bail!("`stream` type mismatch")
    };
    let stream = StreamReader::new(&mut store, Producer(items.fuse()))?;
    let stream = stream.try_into_stream_any(store)?;
    Ok(component::Val::Stream(stream))
}

/// Creates `future` of type `ty` in `store`
pub(super) fn lift_future<T>(
    store: StoreContextMut<'_, T>,
    ty: &Type,
    future: FutureAny,
) -> wasmtime::Result<component::Val> {
    let Type::Future(ty) = ty else {
        bail!("value type mismatch")
    };
    with_payload!(
        ty.ty(),
        P => new_future::<P, T>(store, future),
        bail!("`future` payload type not supported")
    )
}

/// Creates `stream` of type `ty` in `store`
pub(super) fn lift_stream<T>(
    store: StoreContextMut<'_, T>,
    ty: &Type,
    stream: StreamAny,
) -> wasmtime::Result<component::Val> {
    let Type::Stream(ty) = ty else {
        bail!("value type mismatch")
    };
    let Some(payload) = ty.ty() else {
        bail!("`stream` without a payload type not supported")
    };
    with_payload!(
        @some payload,
        P => new_stream::<P, T>(store, stream),
        bail!("`stream` payload type not supported")
    )
}

/// Closes the `future`s and `stream`s `vs`
pub(super) fn close<T>(mut store: StoreContextMut<'_, T>, vs: Vec<component::Val>) {
    for v in vs {
        let res = match v {
            component::Val::Future(mut v) => v.close(&mut store),
            component::Val::Stream(mut v) => v.close(&mut store),
            _ => Ok(()),
        };
        if let Err(err) = res {
            warn!(?err, "failed to close lifted value");
        }
    }
}

/// Pipes `future` of type `ty`, closing it on failure
pub(super) fn lower_future<T>(
    mut store: StoreContextMut<'_, T>,
    ty: &Type,
    future: &mut component::FutureAny,
) -> wasmtime::Result<FutureAny> {
    let res = match ty {
        Type::Future(ty) => with_payload!(
            ty.ty(),
            P => pipe_future::<P, T>(store.as_context_mut(), future.clone()),
            Err(format_err!(
                "`future` payload type not supported, only primitive types and `string` are"
            ))
        ),
        _ => Err(format_err!("value type mismatch")),
    };
    if res.is_err()
        && let Err(err) = future.close(store)
    {
        warn!(?err, "failed to close `future`");
    }
    res
}

/// Pipes `stream` of type `ty`, closing it on failure
pub(super) fn lower_stream<T>(
    mut store: StoreContextMut<'_, T>,
    ty: &Type,
    stream: &mut component::StreamAny,
) -> wasmtime::Result<StreamAny> {
    let res = match ty {
        Type::Stream(ty) => match ty.ty() {
            Some(payload) => with_payload!(
                @some payload,
                P => pipe_stream::<P, T>(store.as_context_mut(), stream.clone()),
                Err(format_err!(
                    "`stream` payload type not supported, only primitive types and `string` are"
                ))
            ),
            None => Err(format_err!("`stream` without a payload type not supported")),
        },
        _ => Err(format_err!("value type mismatch")),
    };
    if res.is_err()
        && let Err(err) = stream.close(store)
    {
        warn!(?err, "failed to close `stream`");
    }
    res
}
