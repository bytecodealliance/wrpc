use core::future::ready;
use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::BytesMut;
use futures::stream;
use futures::{Stream, StreamExt as _, TryFutureExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use wasmtime::bail;
use wasmtime::component::types::{FutureType, StreamType};
use wasmtime::component::{
    Accessor, Destination, FutureAny, FutureConsumer, FutureReader, Lift, Lower, Source, StreamAny,
    StreamConsumer, StreamProducer, StreamReader, StreamResult, Type, Val, VecBuffer,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wrpc_transport::{
    Decode, Encode, FutureDecoder, FutureEncoder, READ_CHUNK_SIZE, Receive, StreamDecoder,
    StreamEncoder, Transmit,
};

use crate::codec::Partial;
use crate::{ValDecoder, WithStore, WrpcView};

impl<T: 'static> WithStore for &Accessor<T> {
    type Data = T;

    fn with_store<R>(&mut self, f: impl FnOnce(StoreContextMut<'_, T>) -> R) -> std::io::Result<R> {
        Ok(self.with(|mut store| f(store.as_context_mut())))
    }
}

macro_rules! with_payload {
    ($ty:expr, $p:ident => $body:expr, $unsupported:expr) => {
        match $ty {
            None => {
                type $p = ();
                $body
            }
            Some(ref ty) => with_payload!(@some ty, $p => $body, $unsupported),
        }
    };
    (@some $ty:expr, $p:ident => $body:expr, $unsupported:expr) => {
        match $ty {
            Type::Bool => {
                type $p = bool;
                $body
            }
            Type::S8 => {
                type $p = i8;
                $body
            }
            Type::U8 => {
                type $p = u8;
                $body
            }
            Type::S16 => {
                type $p = i16;
                $body
            }
            Type::U16 => {
                type $p = u16;
                $body
            }
            Type::S32 => {
                type $p = i32;
                $body
            }
            Type::U32 => {
                type $p = u32;
                $body
            }
            Type::S64 => {
                type $p = i64;
                $body
            }
            Type::U64 => {
                type $p = u64;
                $body
            }
            Type::Float32 => {
                type $p = f32;
                $body
            }
            Type::Float64 => {
                type $p = f64;
                $body
            }
            Type::Char => {
                type $p = char;
                $body
            }
            Type::String => {
                type $p = String;
                $body
            }
            _ => $unsupported,
        }
    };
}

fn pipe_future<P, T>(
    store: StoreContextMut<'_, T>,
    future: FutureAny,
) -> wasmtime::Result<Box<dyn Transmit + Send>>
where
    P: Encode + Lift + Lower + Send + Sync + 'static,
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
    let item = rx.map_err(|_| std::io::Error::other("future dropped without a value"));
    Ok(Box::new(FutureEncoder::pending(item)))
}

fn pipe_stream<P, T>(
    store: StoreContextMut<'_, T>,
    stream: StreamAny,
) -> wasmtime::Result<Box<dyn Transmit + Send>>
where
    P: Encode + Lift + Lower + Send + Sync + 'static,
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
    let items = stream::poll_fn(move |cx| rx.poll_recv(cx));
    Ok(Box::new(StreamEncoder::pending(items)))
}

fn pending_future<P, D>(
    mut store: StoreContextMut<'_, D>,
) -> wasmtime::Result<(Val, Box<dyn Receive + Send>)>
where
    P: Decode + Lift + Lower + Send + Sync + 'static,
    D: 'static,
{
    let (r, v) = FutureDecoder::<P>::pending();
    let fut = FutureReader::new(&mut store, async { wasmtime::error::Ok(v.await) })?;
    let fut = fut.try_into_future_any(store)?;
    Ok((Val::Future(fut), Box::new(r)))
}

fn ready_future<P, D>(mut store: StoreContextMut<'_, D>, v: P) -> wasmtime::Result<Val>
where
    P: Lift + Lower + Send + Sync + 'static,
    D: 'static,
{
    let fut = FutureReader::new(&mut store, ready(wasmtime::error::Ok(v)))?;
    let fut = fut.try_into_future_any(store)?;
    Ok(Val::Future(fut))
}

struct Producer<P>(Option<Pin<Box<dyn Stream<Item = Vec<P>> + Send>>>);

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
        if let Some(rx) = &mut self.0 {
            match rx.poll_next_unpin(cx) {
                Poll::Ready(Some(chunk)) => {
                    dst.set_buffer(chunk.into());
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                Poll::Ready(None) => {
                    self.0 = None;
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Pending => {}
            }
        }
        if finish {
            Poll::Ready(Ok(StreamResult::Cancelled))
        } else {
            Poll::Pending
        }
    }
}

fn pending_stream<P, D>(
    mut store: StoreContextMut<'_, D>,
) -> wasmtime::Result<(Val, Box<dyn Receive + Send>)>
where
    P: Decode + Lift + Lower + Send + Sync + 'static,
    D: 'static,
{
    let (r, rx) = StreamDecoder::<P>::pending();
    let stream = StreamReader::new(&mut store, Producer(Some(Box::pin(rx))))?;
    let stream = stream.try_into_stream_any(store)?;
    Ok((Val::Stream(stream), Box::new(r)))
}

fn ready_stream<P, D>(mut store: StoreContextMut<'_, D>, vs: Vec<P>) -> wasmtime::Result<Val>
where
    P: Lift + Lower + Send + Sync + Unpin + 'static,
    D: 'static,
{
    let stream = StreamReader::new(&mut store, vs)?;
    let stream = stream.try_into_stream_any(store)?;
    Ok(Val::Stream(stream))
}

pub(super) fn encode_future<T>(
    store: StoreContextMut<'_, T>,
    future: &FutureAny,
    ty: &FutureType,
) -> wasmtime::Result<Box<dyn Transmit + Send>> {
    with_payload!(
        ty.ty(),
        P => pipe_future::<P, T>(store, future.clone()),
        {
            future.clone().close(store)?;
            bail!("`future` payload type not supported, only primitive types and `string` are")
        }
    )
}

pub(super) fn encode_stream<T>(
    store: StoreContextMut<'_, T>,
    stream: &StreamAny,
    ty: &StreamType,
) -> wasmtime::Result<Box<dyn Transmit + Send>> {
    let Some(payload) = ty.ty() else {
        stream.clone().close(store)?;
        bail!("`stream` without a payload type not supported")
    };
    with_payload!(
        @some payload,
        P => pipe_stream::<P, T>(store, stream.clone()),
        {
            stream.clone().close(store)?;
            bail!("`stream` payload type not supported, only primitive types and `string` are")
        }
    )
}

impl<T> ValDecoder<'_, T>
where
    T: WithStore<Data: WrpcView>,
{
    pub(super) fn decode_future(
        &mut self,
        src: &mut BytesMut,
        path: &[usize],
        state: &mut Option<Box<Partial>>,
        ty: &FutureType,
    ) -> std::io::Result<Option<Val>> {
        with_payload!(
            ty.ty(),
            P => self.decode_async::<<P as Decode>::Decoder>(
                src,
                path,
                state,
                "future",
                pending_future::<P, _>,
                ready_future::<P, _>,
            ),
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`future` payload type not supported, only primitive types and `string` are",
            ))
        )
    }

    pub(super) fn decode_stream(
        &mut self,
        src: &mut BytesMut,
        path: &[usize],
        state: &mut Option<Box<Partial>>,
        ty: &StreamType,
    ) -> std::io::Result<Option<Val>> {
        let Some(payload) = ty.ty() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`stream` without a payload type not supported",
            ));
        };
        with_payload!(
            @some payload,
            P => self.decode_async::<<P as Decode>::ListDecoder>(
                src,
                path,
                state,
                "stream",
                pending_stream::<P, _>,
                ready_stream::<P, _>,
            ),
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`stream` payload type not supported, only primitive types and `string` are",
            ))
        )
    }
}
