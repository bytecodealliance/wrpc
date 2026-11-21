use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, Fuse};
use futures::{FutureExt as _, StreamExt as _, TryFutureExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use tracing::warn;
use wasmtime::component::{
    self, Destination, FutureConsumer, FutureReader, Lift, Lower, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult, Type, VecBuffer,
};
use wasmtime::{AsContextMut, StoreContextMut, bail, format_err};

use crate::Val;

const READ_CHUNK_SIZE: usize = 8192;

/// Payload of a `future` or `stream`, a static type as Wasmtime requires
trait Payload: Lift + Lower + Send + Sync + Sized + 'static {
    fn from_val(v: Option<Val>) -> std::io::Result<Self>;

    fn into_val(self) -> Option<Val>;
}

impl Payload for () {
    fn from_val(_: Option<Val>) -> std::io::Result<Self> {
        Ok(())
    }

    fn into_val(self) -> Option<Val> {
        None
    }
}

macro_rules! impl_payload {
    ($($t:ty => $v:ident),+ $(,)?) => {
        $(
            impl Payload for $t {
                fn from_val(v: Option<Val>) -> std::io::Result<Self> {
                    let Some(Val::$v(v)) = v else {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "payload type mismatch",
                        ));
                    };
                    Ok(v)
                }

                fn into_val(self) -> Option<Val> {
                    Some(Val::$v(self))
                }
            }
        )+
    };
}

impl_payload!(
    bool => Bool,
    i8 => S8,
    u8 => U8,
    i16 => S16,
    u16 => U16,
    i32 => S32,
    u32 => U32,
    i64 => S64,
    u64 => U64,
    f32 => Float32,
    f64 => Float64,
    char => Char,
    String => String,
);

/// Evaluates `$body` with `$p` set to the [`Payload`] of type `$ty`
macro_rules! with_payload {
    ($ty:expr, $p:ident => $body:expr, $unsupported:expr) => {
        match $ty {
            None => with_payload!(@ty (), $p => $body),
            Some(Type::Bool) => with_payload!(@ty bool, $p => $body),
            Some(Type::S8) => with_payload!(@ty i8, $p => $body),
            Some(Type::U8) => with_payload!(@ty u8, $p => $body),
            Some(Type::S16) => with_payload!(@ty i16, $p => $body),
            Some(Type::U16) => with_payload!(@ty u16, $p => $body),
            Some(Type::S32) => with_payload!(@ty i32, $p => $body),
            Some(Type::U32) => with_payload!(@ty u32, $p => $body),
            Some(Type::S64) => with_payload!(@ty i64, $p => $body),
            Some(Type::U64) => with_payload!(@ty u64, $p => $body),
            Some(Type::Float32) => with_payload!(@ty f32, $p => $body),
            Some(Type::Float64) => with_payload!(@ty f64, $p => $body),
            Some(Type::Char) => with_payload!(@ty char, $p => $body),
            Some(Type::String) => with_payload!(@ty String, $p => $body),
            Some(..) => $unsupported,
        }
    };
    (@ty $t:ty, $p:ident => $body:expr) => {{
        type $p = $t;
        $body
    }};
}

fn pipe_future<P: Payload, T>(
    store: StoreContextMut<'_, T>,
    future: component::FutureAny,
) -> wasmtime::Result<Val> {
    struct Consumer<P>(Option<oneshot::Sender<P>>);

    impl<P: Payload, D> FutureConsumer<D> for Consumer<P> {
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

    let (tx, rx) = oneshot::channel::<P>();
    let future = future.try_into_future_reader::<P>()?;
    future.pipe(store, Consumer(Some(tx)))?;
    let v = rx
        .map_ok(P::into_val)
        .map_err(|_| std::io::Error::other("future dropped without a value"));
    Ok(Val::Future(Box::pin(v)))
}

fn pipe_stream<P: Payload, T>(
    store: StoreContextMut<'_, T>,
    stream: component::StreamAny,
) -> wasmtime::Result<Val> {
    struct Consumer<P>(PollSender<Vec<P>>);

    impl<P: Payload, D> StreamConsumer<D> for Consumer<P> {
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

    let (tx, mut rx) = mpsc::channel::<Vec<P>>(1);
    let stream = stream.try_into_stream_reader::<P>()?;
    stream.pipe(store, Consumer(PollSender::new(tx)))?;
    let items = stream::poll_fn(move |cx| rx.poll_recv(cx))
        .map(|vs| vs.into_iter().filter_map(P::into_val).collect());
    Ok(Val::Stream(Box::pin(items)))
}

fn new_future<P: Payload, T>(
    mut store: StoreContextMut<'_, T>,
    future: BoxFuture<'static, std::io::Result<Option<Val>>>,
) -> wasmtime::Result<component::Val> {
    let future = future.map(|v| {
        let v = v?;
        P::from_val(v)
    });
    let future = FutureReader::new(&mut store, future)?;
    let future = future.try_into_future_any(store)?;
    Ok(component::Val::Future(future))
}

struct Producer<P>(Fuse<BoxStream<'static, Vec<Val>>>, PhantomData<fn() -> P>);

impl<P: Payload, D> StreamProducer<D> for Producer<P> {
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
                let chunk = chunk
                    .into_iter()
                    .map(|v| P::from_val(Some(v)))
                    .collect::<std::io::Result<Vec<_>>>()?;
                dst.set_buffer(chunk.into());
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn new_stream<P: Payload, T>(
    mut store: StoreContextMut<'_, T>,
    stream: BoxStream<'static, Vec<Val>>,
) -> wasmtime::Result<component::Val> {
    let stream = StreamReader::new(&mut store, Producer::<P>(stream.fuse(), PhantomData))?;
    let stream = stream.try_into_stream_any(store)?;
    Ok(component::Val::Stream(stream))
}

/// Creates `future` of type `ty` in `store`
pub(super) fn lift_future<T>(
    store: StoreContextMut<'_, T>,
    ty: &Type,
    future: BoxFuture<'static, std::io::Result<Option<Val>>>,
) -> wasmtime::Result<component::Val> {
    let Type::Future(ty) = ty else {
        bail!("value type mismatch")
    };
    with_payload!(
        ty.ty(),
        P => new_future::<P, T>(store, future),
        bail!("`future` payload type not supported, only primitive types and `string` are")
    )
}

/// Creates `stream` of type `ty` in `store`
pub(super) fn lift_stream<T>(
    store: StoreContextMut<'_, T>,
    ty: &Type,
    stream: BoxStream<'static, Vec<Val>>,
) -> wasmtime::Result<component::Val> {
    let Type::Stream(ty) = ty else {
        bail!("value type mismatch")
    };
    let Some(payload) = ty.ty() else {
        bail!("`stream` without a payload type not supported")
    };
    with_payload!(
        Some(payload),
        P => new_stream::<P, T>(store, stream),
        bail!("`stream` payload type not supported, only primitive types and `string` are")
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
) -> wasmtime::Result<Val> {
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
) -> wasmtime::Result<Val> {
    let res = match ty {
        Type::Stream(ty) => match ty.ty() {
            None => Err(format_err!("`stream` without a payload type not supported")),
            payload => with_payload!(
                payload,
                P => pipe_stream::<P, T>(store.as_context_mut(), stream.clone()),
                Err(format_err!(
                    "`stream` payload type not supported, only primitive types and `string` are"
                ))
            ),
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
