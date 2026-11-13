use core::any::Any;
use core::borrow::Borrow;
use core::future::{pending, ready};
use core::iter::{self, zip};
use core::mem;
use core::ops::{BitOrAssign, Deref, Shl};
use core::pin::Pin;
use core::task::{Context, Poll};

use std::collections::HashSet;

use bytes::{Buf as _, BufMut as _, BytesMut};
use futures::StreamExt as _;
use futures::stream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Decoder, Encoder, FramedRead};
use tokio_util::sync::PollSender;
use tracing::{debug, error, instrument, trace, warn};
use uuid::Uuid;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecEncoderBytes, DEFAULT_MAX_INITIAL_CAPACITY,
    Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64, Leb128DecoderU8, Leb128DecoderU16,
    Leb128DecoderU32, Leb128DecoderU64, Leb128Encoder, Utf8Codec,
};
use wasmtime::bail;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{
    Accessor, Destination, FutureAny, FutureConsumer, FutureReader, Lift, Lower, Source, StreamAny,
    StreamConsumer, StreamProducer, StreamReader, StreamResult, Type, Val, VecBuffer,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};
use wrpc_transport::frame::{Incoming, Outgoing};
use wrpc_transport::{
    Decode, Deferred, DeferredFn, DeferredWrite as _, Encode, READ_CHUNK_SIZE, handle_deferred,
};

use crate::{ResourceTypes, WrpcView};

/// Synchronous access to a store, which is either borrowed exclusively or reachable through an
/// [`Accessor`].
///
/// Wasmtime only provides an [`Accessor`] to host functions implementing `async func` imports,
/// host functions implementing other imports hold a [`StoreContextMut`] across `await` points.
pub trait WithStore {
    type Data: 'static;

    fn with_store<R>(&mut self, f: impl FnOnce(StoreContextMut<'_, Self::Data>) -> R) -> R;
}

impl<T: 'static> WithStore for StoreContextMut<'_, T> {
    type Data = T;

    fn with_store<R>(&mut self, f: impl FnOnce(StoreContextMut<'_, T>) -> R) -> R {
        f(self.as_context_mut())
    }
}

impl<T: 'static> WithStore for &Accessor<T> {
    type Data = T;

    fn with_store<R>(&mut self, f: impl FnOnce(StoreContextMut<'_, T>) -> R) -> R {
        self.with(|mut store| f(store.as_context_mut()))
    }
}

pub struct ValEncoder<'a, T: 'static> {
    pub store: StoreContextMut<'a, T>,
    pub ty: &'a Type,
    pub resources: &'a ResourceTypes,
    pub deferred: Option<DeferredFn<Outgoing>>,
}

impl<T> ValEncoder<'_, T> {
    #[must_use]
    pub fn new<'a>(
        store: StoreContextMut<'a, T>,
        ty: &'a Type,
        resources: &'a ResourceTypes,
    ) -> ValEncoder<'a, T> {
        ValEncoder {
            store,
            ty,
            resources,
            deferred: None,
        }
    }

    pub fn with_type<'a>(&'a mut self, ty: &'a Type) -> ValEncoder<'a, T> {
        ValEncoder {
            store: self.store.as_context_mut(),
            ty,
            resources: self.resources,
            deferred: None,
        }
    }
}

impl<T> Deferred<Outgoing> for ValEncoder<'_, T> {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

fn find_enum_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    names: impl IntoIterator<Item = &'a str>,
    discriminant: &str,
) -> wasmtime::Result<T> {
    zip(iter, names)
        .find_map(|(i, name)| (name == discriminant).then_some(i))
        .context("unknown enum discriminant")
}

fn find_variant_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    cases: impl IntoIterator<Item = Case<'a>>,
    discriminant: &str,
) -> wasmtime::Result<(T, Option<Type>)> {
    zip(iter, cases)
        .find_map(|(i, Case { name, ty })| (name == discriminant).then_some((i, ty)))
        .context("unknown variant discriminant")
}

#[inline]
fn flag_bits<'a, T: BitOrAssign + Shl<u8, Output = T> + From<u8>>(
    names: impl IntoIterator<Item = &'a str>,
    flags: impl IntoIterator<Item = &'a str>,
) -> T {
    let mut v = T::from(0);
    let flags: HashSet<&str> = flags.into_iter().collect();
    for (i, name) in zip(0u8.., names) {
        if flags.contains(name) {
            v |= T::from(1) << i;
        }
    }
    v
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

fn encode_future<P, T>(
    store: StoreContextMut<'_, T>,
    future: FutureAny,
    offset: usize,
) -> wasmtime::Result<DeferredFn<Outgoing>>
where
    P: Encode + Lift + Lower + Send + Sync + 'static,
    std::io::Error: From<<P::Encoder as Encoder<P>>::Error>,
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
    Ok(Box::new(move |w, path| {
        let w = w.index(path)?;
        Ok(Box::pin(async move {
            let v = rx
                .await
                .map_err(|_| std::io::Error::other("future dropped without a value"))?;
            w.write_future(offset, ready(v)).await
        }))
    }))
}

fn encode_stream<P, T>(
    store: StoreContextMut<'_, T>,
    stream: StreamAny,
    offset: usize,
) -> wasmtime::Result<DeferredFn<Outgoing>>
where
    P: Encode + Lift + Lower + Send + Sync + 'static,
    std::io::Error: From<<P::Encoder as Encoder<P>>::Error>,
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
    Ok(Box::new(move |w, path| {
        let w = w.index(path)?;
        Ok(Box::pin(async move {
            w.write_stream(offset, stream::poll_fn(move |cx| rx.poll_recv(cx)))
                .await
        }))
    }))
}

fn report(errors: &mpsc::Sender<std::io::Error>, err: std::io::Error) {
    if let Err(err) = errors.try_send(err) {
        debug!(err = %err.into_inner(), "failed to receive asynchronous value");
    }
}

fn poll_chunk<D, P>(
    rx: &mut FramedRead<Incoming, D>,
    cx: &mut Context<'_>,
) -> Poll<std::io::Result<Option<D::Item>>>
where
    D: Decoder<Item: Deref<Target = [P]>>,
    std::io::Error: From<D::Error>,
{
    let chunk = match rx.poll_next_unpin(cx) {
        Poll::Ready(Some(Ok(chunk))) => chunk,
        Poll::Ready(Some(Err(err))) => return Poll::Ready(Err(err.into())),
        Poll::Pending => return Poll::Pending,
        Poll::Ready(None) => {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "stream end missing",
            )));
        }
    };
    if !chunk.is_empty() {
        Poll::Ready(Ok(Some(chunk)))
    } else if rx.read_buffer().is_empty() {
        Poll::Ready(Ok(None))
    } else {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "received data after stream end",
        )))
    }
}

fn pending_future<P, D>(
    mut store: StoreContextMut<'_, D>,
    r: Incoming,
    errors: mpsc::Sender<std::io::Error>,
) -> wasmtime::Result<Val>
where
    P: Decode + Lift + Lower + Send + Sync + 'static,
    D: 'static,
    std::io::Error: From<<P::Decoder as Decoder>::Error>,
{
    let fut = FutureReader::new(&mut store, async move {
        let mut rx = FramedRead::new(r, P::Decoder::default());
        let err = match rx.next().await {
            Some(Ok(v)) if rx.read_buffer().is_empty() => return wasmtime::error::Ok(v),
            Some(Ok(..)) => std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "received data after future value",
            ),
            Some(Err(err)) => err.into(),
            None => std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "future value missing"),
        };
        report(&errors, err);
        drop(rx);
        pending().await
    })?;
    let fut = fut.try_into_future_any(store)?;
    Ok(Val::Future(fut))
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

struct Producer<P: Decode>(
    Option<(
        FramedRead<Incoming, P::ListDecoder>,
        mpsc::Sender<std::io::Error>,
    )>,
);

impl<P, D> StreamProducer<D> for Producer<P>
where
    P: Decode<ListDecoder: Send> + Lift + Lower + Send + Sync + 'static,
    std::io::Error: From<<P::ListDecoder as Decoder>::Error>,
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
        if let Some((rx, errors)) = &mut self.0 {
            match poll_chunk(rx, cx) {
                Poll::Ready(Ok(Some(chunk))) => {
                    dst.set_buffer(chunk.into());
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                Poll::Ready(Ok(None)) => {
                    self.0 = None;
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Err(err)) => {
                    report(errors, err);
                    self.0 = None;
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
    r: Incoming,
    errors: mpsc::Sender<std::io::Error>,
) -> wasmtime::Result<Val>
where
    P: Decode<ListDecoder: Send> + Lift + Lower + Send + Sync + 'static,
    D: 'static,
    std::io::Error: From<<P::ListDecoder as Decoder>::Error>,
{
    let rx = FramedRead::new(r, P::ListDecoder::default());
    let stream = StreamReader::new(&mut store, Producer::<P>(Some((rx, errors))))?;
    let stream = stream.try_into_stream_any(store)?;
    Ok(Val::Stream(stream))
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

#[cfg(feature = "wasmtime-wasi")]
fn pending_input_stream<T: WrpcView>(
    store: StoreContextMut<'_, T>,
    r: Incoming,
    errors: mpsc::Sender<std::io::Error>,
) -> wasmtime::Result<Val> {
    let rx = FramedRead::new(r, <wasm_tokio::CoreVecDecoderBytes>::default());
    let chunks = stream::try_unfold((rx, errors), |(mut rx, errors)| async move {
        match core::future::poll_fn(|cx| poll_chunk(&mut rx, cx)).await {
            Ok(chunk) => Ok(chunk.map(|chunk| (chunk, (rx, errors)))),
            Err(err) => {
                let kind = err.kind();
                report(&errors, err);
                Err(std::io::Error::from(kind))
            }
        }
    });
    push_input_stream(
        store,
        Box::new(wasmtime_wasi::p2::pipe::AsyncReadStream::new(
            tokio_util::io::StreamReader::new(chunks),
        )),
    )
}

#[cfg(feature = "wasmtime-wasi")]
fn ready_input_stream<T: WrpcView>(
    store: StoreContextMut<'_, T>,
    buf: bytes::Bytes,
) -> wasmtime::Result<Val> {
    push_input_stream(
        store,
        Box::new(wasmtime_wasi::p2::pipe::MemoryInputPipe::new(buf)),
    )
}

// The stream must be typed as `DynInputStream` (the host resource type),
// otherwise the resulting resource handle carries the concrete reader type
// and fails the guest's `own<input-stream>` type check.
#[cfg(feature = "wasmtime-wasi")]
fn push_input_stream<T: WrpcView>(
    mut store: StoreContextMut<'_, T>,
    stream: wasmtime_wasi::p2::DynInputStream,
) -> wasmtime::Result<Val> {
    let stream = store.data_mut().wrpc().table.push(stream)?;
    let stream = stream.try_into_resource_any(store)?;
    Ok(Val::Resource(stream))
}

impl<T> Encoder<&Val> for ValEncoder<'_, T>
where
    T: WrpcView,
{
    type Error = wasmtime::Error;

    #[allow(clippy::too_many_lines)]
    #[instrument(level = "trace", skip(self))]
    fn encode(&mut self, v: &Val, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match (v, self.ty) {
            (Val::Bool(v), Type::Bool) => {
                dst.reserve(1);
                dst.put_u8((*v).into());
                Ok(())
            }
            (Val::S8(v), Type::S8) => {
                dst.reserve(1);
                dst.put_i8(*v);
                Ok(())
            }
            (Val::U8(v), Type::U8) => {
                dst.reserve(1);
                dst.put_u8(*v);
                Ok(())
            }
            (Val::S16(v), Type::S16) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s16"),
            (Val::U16(v), Type::U16) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u16"),
            (Val::S32(v), Type::S32) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s32"),
            (Val::U32(v), Type::U32) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u32"),
            (Val::S64(v), Type::S64) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s64"),
            (Val::U64(v), Type::U64) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u64"),
            (Val::Float32(v), Type::Float32) => {
                F32Codec.encode(*v, dst).context("failed to encode f32")
            }
            (Val::Float64(v), Type::Float64) => {
                F64Codec.encode(*v, dst).context("failed to encode f64")
            }
            (Val::Char(v), Type::Char) => {
                Utf8Codec.encode(*v, dst).context("failed to encode char")
            }
            (Val::String(v), Type::String) => CoreNameEncoder
                .encode(v.as_str(), dst)
                .context("failed to encode string"),
            (Val::List(vs), Type::List(ty)) => {
                let ty = ty.ty();
                let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
                dst.reserve(5 + vs.len());
                Leb128Encoder
                    .encode(n, dst)
                    .context("failed to encode list length")?;
                let mut deferred = Vec::with_capacity(vs.len());
                for v in vs {
                    let mut enc = self.with_type(&ty);
                    enc.encode(v, dst)
                        .context("failed to encode list element")?;
                    deferred.push(enc.take_deferred());
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w, path| handle_deferred(w, path, deferred)));
                }
                Ok(())
            }
            (Val::Record(vs), Type::Record(ty)) => {
                if vs.len() != ty.fields().len() {
                    bail!(
                        "record has {} fields, expected {}",
                        vs.len(),
                        ty.fields().len()
                    );
                }
                dst.reserve(vs.len());
                let mut deferred = Vec::with_capacity(vs.len());
                for ((name, v), Field { ref ty, .. }) in zip(vs, ty.fields()) {
                    let mut enc = self.with_type(ty);
                    enc.encode(v, dst)
                        .with_context(|| format!("failed to encode `{name}` field"))?;
                    deferred.push(enc.take_deferred());
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w, path| handle_deferred(w, path, deferred)));
                }
                Ok(())
            }
            (Val::Tuple(vs), Type::Tuple(ty)) => {
                if vs.len() != ty.types().len() {
                    bail!(
                        "tuple has {} elements, expected {}",
                        vs.len(),
                        ty.types().len()
                    );
                }
                dst.reserve(vs.len());
                let mut deferred = Vec::with_capacity(vs.len());
                for (v, ref ty) in zip(vs, ty.types()) {
                    let mut enc = self.with_type(ty);
                    enc.encode(v, dst)
                        .context("failed to encode tuple element")?;
                    deferred.push(enc.take_deferred());
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w, path| handle_deferred(w, path, deferred)));
                }
                Ok(())
            }
            (Val::Variant(discriminant, v), Type::Variant(ty)) => {
                let cases = ty.cases();
                let ty = match cases.len() {
                    ..=0x0000_00ff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u8.., cases, discriminant)?;
                        dst.reserve(2 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u16.., cases, discriminant)?;
                        dst.reserve(3 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u32.., cases, discriminant)?;
                        dst.reserve(4 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u32.., cases, discriminant)?;
                        dst.reserve(5 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x1_0000_0000.. => bail!("case count does not fit in u32"),
                };
                if let Some(v) = v {
                    let ty = ty.context("type missing for variant")?;
                    let mut enc = self.with_type(&ty);
                    enc.encode(v, dst)
                        .context("failed to encode variant value")?;
                    if let Some(f) = enc.take_deferred() {
                        self.deferred = Some(f);
                    }
                } else if ty.is_some() {
                    bail!("payload missing for variant `{discriminant}`");
                }
                Ok(())
            }
            (Val::Enum(discriminant), Type::Enum(ty)) => {
                let names = ty.names();
                match names.len() {
                    ..=0x0000_00ff => {
                        let discriminant = find_enum_discriminant(0u8.., names, discriminant)?;
                        dst.reserve(2);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let discriminant = find_enum_discriminant(0u16.., names, discriminant)?;
                        dst.reserve(3);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let discriminant = find_enum_discriminant(0u32.., names, discriminant)?;
                        dst.reserve(4);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let discriminant = find_enum_discriminant(0u32.., names, discriminant)?;
                        dst.reserve(5);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x1_0000_0000.. => bail!("name count does not fit in u32"),
                }
                Ok(())
            }
            (Val::Option(None), Type::Option(_)) => {
                dst.reserve(1);
                dst.put_u8(0);
                Ok(())
            }
            (Val::Option(Some(v)), Type::Option(ty)) => {
                dst.reserve(2);
                dst.put_u8(1);
                let ty = ty.ty();
                let mut enc = self.with_type(&ty);
                enc.encode(v, dst)
                    .context("failed to encode `option::some` value")?;
                if let Some(f) = enc.take_deferred() {
                    self.deferred = Some(f);
                }
                Ok(())
            }
            (Val::Result(v), Type::Result(ty)) => match v {
                Ok(v) => match (v, ty.ok()) {
                    (Some(v), Some(ty)) => {
                        dst.reserve(2);
                        dst.put_u8(0);
                        let mut enc = self.with_type(&ty);
                        enc.encode(v, dst)
                            .context("failed to encode `result::ok` value")?;
                        if let Some(f) = enc.take_deferred() {
                            self.deferred = Some(f);
                        }
                        Ok(())
                    }
                    (Some(_v), None) => bail!("`result::ok` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::ok` value missing"),
                    (None, None) => {
                        dst.reserve(1);
                        dst.put_u8(0);
                        Ok(())
                    }
                },
                Err(v) => match (v, ty.err()) {
                    (Some(v), Some(ty)) => {
                        dst.reserve(2);
                        dst.put_u8(1);
                        let mut enc = self.with_type(&ty);
                        enc.encode(v, dst)
                            .context("failed to encode `result::err` value")?;
                        if let Some(f) = enc.take_deferred() {
                            self.deferred = Some(f);
                        }
                        Ok(())
                    }
                    (Some(_v), None) => bail!("`result::err` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::err` value missing"),
                    (None, None) => {
                        dst.reserve(1);
                        dst.put_u8(1);
                        Ok(())
                    }
                },
            },
            (Val::Flags(vs), Type::Flags(ty)) => {
                let names = ty.names();
                let vs = vs.iter().map(String::as_str);
                match names.len() {
                    ..=8 => {
                        dst.reserve(1);
                        dst.put_u8(flag_bits(names, vs));
                    }
                    9..=16 => {
                        dst.reserve(2);
                        dst.put_u16_le(flag_bits(names, vs));
                    }
                    17..=24 => {
                        dst.reserve(3);
                        dst.put_slice(&u32::to_le_bytes(flag_bits(names, vs))[..3]);
                    }
                    25..=32 => {
                        dst.reserve(4);
                        dst.put_u32_le(flag_bits(names, vs));
                    }
                    33..=40 => {
                        dst.reserve(5);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..5]);
                    }
                    41..=48 => {
                        dst.reserve(6);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..6]);
                    }
                    49..=56 => {
                        dst.reserve(7);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..7]);
                    }
                    57..=64 => {
                        dst.reserve(8);
                        dst.put_u64_le(flag_bits(names, vs));
                    }
                    65..=72 => {
                        dst.reserve(9);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..9]);
                    }
                    73..=80 => {
                        dst.reserve(10);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..10]);
                    }
                    81..=88 => {
                        dst.reserve(11);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..11]);
                    }
                    89..=96 => {
                        dst.reserve(12);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..12]);
                    }
                    97..=104 => {
                        dst.reserve(13);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..13]);
                    }
                    105..=112 => {
                        dst.reserve(14);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..14]);
                    }
                    113..=120 => {
                        dst.reserve(15);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..15]);
                    }
                    121..=128 => {
                        dst.reserve(16);
                        dst.put_u128_le(flag_bits(names, vs));
                    }
                    bits @ 129.. => {
                        let mut cap = bits / 8;
                        if bits % 8 != 0 {
                            cap = cap.saturating_add(1);
                        }
                        let mut buf = vec![0; cap];
                        let flags: HashSet<&str> = vs.into_iter().collect();
                        for (i, name) in names.enumerate() {
                            if flags.contains(name) {
                                buf[i / 8] |= 1 << (i % 8);
                            }
                        }
                        dst.extend_from_slice(&buf);
                    }
                }
                Ok(())
            }
            (Val::Resource(resource), Type::Own(ty) | Type::Borrow(ty)) => {
                #[cfg(feature = "wasmtime-wasi")]
                if *ty
                    == wasmtime::component::ResourceType::host::<wasmtime_wasi::p2::DynInputStream>(
                    )
                    || self.resources.io_streams.contains(ty)
                {
                    let stream = resource
                        .try_into_resource::<wasmtime_wasi::p2::DynInputStream>(&mut self.store)
                        .context("failed to downcast `wasi:io/input-stream`")?;
                    if stream.owned() {
                        let stream = self
                            .store
                            .data_mut()
                            .wrpc()
                            .table
                            .delete(stream)
                            .context("failed to delete input stream")?;
                        let items: Pin<Box<dyn tokio::io::AsyncRead + Send>> =
                            Box::pin(tokio_util::io::StreamReader::new(
                                futures::stream::try_unfold(stream, |mut stream| async move {
                                    loop {
                                        stream.ready().await;
                                        match stream.read(wrpc_transport::READ_CHUNK_SIZE) {
                                            Ok(buf) if buf.is_empty() => {}
                                            Ok(buf) => return Ok(Some((buf, stream))),
                                            Err(wasmtime_wasi::p2::StreamError::Closed) => {
                                                return Ok(None);
                                            }
                                            Err(err) => return Err(std::io::Error::other(err)),
                                        }
                                    }
                                }),
                            ));
                        let mut enc = wrpc_transport::StreamEncoderRead::<
                            Outgoing,
                            { wrpc_transport::READ_CHUNK_SIZE },
                        >::default();
                        enc.encode(items, dst)
                            .context("failed to encode input stream")?;
                        self.deferred = enc.take_deferred();
                    } else {
                        self.store
                            .data_mut()
                            .wrpc()
                            .table
                            .get_mut(&stream)
                            .context("failed to get input stream")?;
                        // NOTE: In order to handle this we'd need to know how many bytes the
                        // receiver has read. That means that some kind of callback would be required from
                        // the receiver. This is not trivial and generally should be a very rare use case.
                        bail!("encoding borrowed `wasi:io/input-stream` not supported yet");
                    }
                    return Ok(());
                }
                if self.resources.guest.contains(ty) {
                    let id = Uuid::now_v7();
                    CoreVecEncoderBytes
                        .encode(id.to_bytes_le().as_slice(), dst)
                        .context("failed to encode resource handle")?;
                    trace!(?id, "store shared resource");
                    if self
                        .store
                        .data_mut()
                        .wrpc()
                        .shared_resources
                        .0
                        .insert(id, *resource)
                        .is_some()
                    {
                        error!(?id, "duplicate resource ID generated");
                    }
                    Ok(())
                } else {
                    bail!("encoding host resources not supported yet")
                }
            }

            (Val::Future(future), Type::Future(ty)) => {
                let offset = dst.len();
                let store = self.store.as_context_mut();
                let deferred = with_payload!(
                    ty.ty(),
                    P => encode_future::<P, T>(store, future.clone(), offset),
                    {
                        future.clone().close(store)?;
                        bail!("`future` payload type not supported, only primitive types and `string` are")
                    }
                )?;
                dst.reserve(1);
                dst.put_u8(0);
                self.deferred = Some(deferred);
                Ok(())
            }
            (Val::Stream(stream), Type::Stream(ty)) => {
                let offset = dst.len();
                let store = self.store.as_context_mut();
                let deferred = match ty.ty() {
                    Some(payload) => with_payload!(
                        @some payload,
                        P => encode_stream::<P, T>(store, stream.clone(), offset),
                        {
                            stream.clone().close(store)?;
                            bail!("`stream` payload type not supported, only primitive types and `string` are")
                        }
                    ),
                    None => {
                        stream.clone().close(store)?;
                        bail!("`stream` without a payload type not supported")
                    }
                }?;
                dst.reserve(1);
                dst.put_u8(0);
                self.deferred = Some(deferred);
                Ok(())
            }
            (_, Type::ErrorContext) => {
                bail!("`error-context` not supported")
            }
            (_, Type::Map(..)) => {
                bail!("maps not supported")
            }
            (_, Type::FixedLengthList(..)) => {
                bail!("fixed-length lists not supported")
            }
            _ => bail!("value type mismatch"),
        }
    }
}

/// Progress of a [`ValDecoder`] within a partially-received value
enum Partial {
    Decoder(Box<dyn Any + Send>),
    Uuid,
    Seq {
        n: usize,
        vs: Vec<Val>,
        elem: Option<Box<Partial>>,
    },
    Case {
        v: Val,
        ty: Type,
        payload: Option<Box<Partial>>,
    },
}

/// Decodes a tuple of [`Val`]s of types `tys`, e.g. function parameters.
///
/// Resources and asynchronous values are created in the store once their encoding is complete.
/// Pending asynchronous values are received from the sub-streams of `rx`, which are indexed as
/// soon as the values are decoded. Failures receiving them are sent on `errors`.
pub struct ValDecoder<'a, T> {
    store: T,
    tys: &'a [Type],
    resources: &'a ResourceTypes,
    rx: &'a Incoming,
    errors: mpsc::Sender<std::io::Error>,
    state: Option<Box<Partial>>,
}

impl<T> ValDecoder<'_, T> {
    #[must_use]
    pub fn new<'a>(
        store: T,
        tys: &'a [Type],
        resources: &'a ResourceTypes,
        rx: &'a Incoming,
        errors: mpsc::Sender<std::io::Error>,
    ) -> ValDecoder<'a, T> {
        ValDecoder {
            store,
            tys,
            resources,
            rx,
            errors,
            state: None,
        }
    }
}

fn decode_status(src: &mut BytesMut, kind: &str) -> std::io::Result<Option<usize>> {
    let Some(&status) = src.first() else {
        src.reserve(1);
        return Ok(None);
    };
    if status > 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid {kind} status byte value `{status}`"),
        ));
    }
    src.advance(1);
    Ok(Some(status.into()))
}

fn decode_discriminant(src: &mut BytesMut) -> std::io::Result<Option<usize>> {
    let Some(v) = Leb128DecoderU32.decode(src)? else {
        return Ok(None);
    };
    let v = usize::try_from(v)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    Ok(Some(v))
}

fn decode_leaf<T>(
    mut dec: T,
    src: &mut BytesMut,
    f: impl FnOnce(T::Item) -> Val,
) -> std::io::Result<Option<Val>>
where
    T: Decoder<Error = std::io::Error>,
{
    let v = dec.decode(src)?;
    Ok(v.map(f))
}

fn decode_with<T>(
    src: &mut BytesMut,
    state: &mut Option<Box<Partial>>,
) -> std::io::Result<Option<T::Item>>
where
    T: Decoder + Default + Send + 'static,
    std::io::Error: From<T::Error>,
{
    let mut dec = match state.take().map(|state| *state) {
        Some(Partial::Decoder(dec)) => dec.downcast().map_or_else(|_| T::default(), |dec| *dec),
        _ => T::default(),
    };
    let v = dec.decode(src)?;
    if v.is_none() {
        *state = Some(Box::new(Partial::Decoder(Box::new(dec))));
    }
    Ok(v)
}

impl<T> ValDecoder<'_, T>
where
    T: WithStore<Data: WrpcView>,
{
    fn decode_seq(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
        len: Option<usize>,
        tys: impl IntoIterator<Item: Borrow<Type>>,
    ) -> std::io::Result<Option<Vec<Val>>> {
        let (n, mut vs, mut elem) = match state.take().map(|state| *state) {
            Some(Partial::Seq { n, vs, elem }) => (n, vs, elem),
            _ => {
                let n = if let Some(n) = len {
                    n
                } else {
                    let Some(n) = Leb128DecoderU32.decode(src)? else {
                        return Ok(None);
                    };
                    usize::try_from(n).unwrap_or(usize::MAX)
                };
                let vs =
                    Vec::with_capacity(n.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<Val>()));
                (n, vs, None)
            }
        };
        for ty in tys.into_iter().take(n).skip(vs.len()) {
            path.push(vs.len());
            let v = self.decode_ty(src, ty.borrow(), path, &mut elem)?;
            path.pop();
            let Some(v) = v else {
                *state = Some(Box::new(Partial::Seq { n, vs, elem }));
                return Ok(None);
            };
            vs.push(v);
        }
        Ok(Some(vs))
    }

    fn decode_case(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
        case: impl FnOnce(&mut BytesMut) -> std::io::Result<Option<(Val, Option<Type>)>>,
    ) -> std::io::Result<Option<Val>> {
        let (mut v, ty, mut payload) = match state.take().map(|state| *state) {
            Some(Partial::Case { v, ty, payload }) => (v, ty, payload),
            _ => {
                let Some((v, ty)) = case(src)? else {
                    return Ok(None);
                };
                let Some(ty) = ty else {
                    return Ok(Some(v));
                };
                (v, ty, None)
            }
        };
        let Some(p) = self.decode_ty(src, &ty, path, &mut payload)? else {
            *state = Some(Box::new(Partial::Case { v, ty, payload }));
            return Ok(None);
        };
        if let Val::Variant(.., x) | Val::Option(x) | Val::Result(Ok(x) | Err(x)) = &mut v {
            *x = Some(Box::new(p));
        }
        Ok(Some(v))
    }

    fn decode_async<D>(
        &mut self,
        src: &mut BytesMut,
        path: &[usize],
        state: &mut Option<Box<Partial>>,
        kind: &str,
        pending: impl FnOnce(
            StoreContextMut<'_, T::Data>,
            Incoming,
            mpsc::Sender<std::io::Error>,
        ) -> wasmtime::Result<Val>,
        ready: impl FnOnce(StoreContextMut<'_, T::Data>, D::Item) -> wasmtime::Result<Val>,
    ) -> std::io::Result<Option<Val>>
    where
        D: Decoder + Default + Send + 'static,
        std::io::Error: From<D::Error>,
    {
        if state.is_none() {
            let Some(status) = decode_status(src, kind)? else {
                return Ok(None);
            };
            if status == 0 {
                let r = self.rx.index(path)?;
                let errors = self.errors.clone();
                let v = self
                    .store
                    .with_store(|store| pending(store, r, errors))
                    .map_err(std::io::Error::other)?;
                return Ok(Some(v));
            }
        }
        let Some(v) = decode_with::<D>(src, state)? else {
            return Ok(None);
        };
        let v = self
            .store
            .with_store(|store| ready(store, v))
            .map_err(std::io::Error::other)?;
        Ok(Some(v))
    }

    #[allow(clippy::too_many_lines)]
    fn decode_ty(
        &mut self,
        src: &mut BytesMut,
        ty: &Type,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
    ) -> std::io::Result<Option<Val>> {
        match ty {
            Type::Bool => decode_leaf(BoolCodec, src, Val::Bool),
            Type::S8 => decode_leaf(S8Codec, src, Val::S8),
            Type::U8 => decode_leaf(U8Codec, src, Val::U8),
            Type::S16 => decode_leaf(Leb128DecoderI16, src, Val::S16),
            Type::U16 => decode_leaf(Leb128DecoderU16, src, Val::U16),
            Type::S32 => decode_leaf(Leb128DecoderI32, src, Val::S32),
            Type::U32 => decode_leaf(Leb128DecoderU32, src, Val::U32),
            Type::S64 => decode_leaf(Leb128DecoderI64, src, Val::S64),
            Type::U64 => decode_leaf(Leb128DecoderU64, src, Val::U64),
            Type::Float32 => decode_leaf(F32Codec, src, Val::Float32),
            Type::Float64 => decode_leaf(F64Codec, src, Val::Float64),
            Type::Char => decode_leaf(Utf8Codec, src, Val::Char),
            Type::String => {
                let v = decode_with::<CoreNameDecoder>(src, state)?;
                Ok(v.map(Val::String))
            }
            Type::List(ty) => {
                let ty = ty.ty();
                let vs = self.decode_seq(src, path, state, None, iter::repeat(&ty))?;
                Ok(vs.map(Val::List))
            }
            Type::Record(ty) => {
                let n = ty.fields().len();
                let tys = ty.fields().map(|Field { ty, .. }| ty);
                let Some(vs) = self.decode_seq(src, path, state, Some(n), tys)? else {
                    return Ok(None);
                };
                let vs = zip(ty.fields(), vs)
                    .map(|(Field { name, .. }, v)| (name.to_string(), v))
                    .collect();
                Ok(Some(Val::Record(vs)))
            }
            Type::Tuple(ty) => {
                let n = ty.types().len();
                let vs = self.decode_seq(src, path, state, Some(n), ty.types())?;
                Ok(vs.map(Val::Tuple))
            }
            Type::Variant(ty) => self.decode_case(src, path, state, |src| {
                let Some(discriminant) = decode_discriminant(src)? else {
                    return Ok(None);
                };
                let Case { name, ty } = ty.cases().nth(discriminant).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown variant discriminant `{discriminant}`"),
                    )
                })?;
                Ok(Some((Val::Variant(name.to_string(), None), ty)))
            }),
            Type::Enum(ty) => {
                let Some(discriminant) = decode_discriminant(src)? else {
                    return Ok(None);
                };
                let name = ty.names().nth(discriminant).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown enum discriminant `{discriminant}`"),
                    )
                })?;
                Ok(Some(Val::Enum(name.to_string())))
            }
            Type::Option(ty) => self.decode_case(src, path, state, |src| {
                let Some(status) = decode_status(src, "option")? else {
                    return Ok(None);
                };
                Ok(Some((Val::Option(None), (status == 1).then(|| ty.ty()))))
            }),
            Type::Result(ty) => self.decode_case(src, path, state, |src| {
                let Some(status) = decode_status(src, "result")? else {
                    return Ok(None);
                };
                Ok(Some(if status == 0 {
                    (Val::Result(Ok(None)), ty.ok())
                } else {
                    (Val::Result(Err(None)), ty.err())
                }))
            }),
            Type::Flags(ty) => {
                let names = ty.names();
                let n = names.len().div_ceil(8).max(1);
                if src.len() < n {
                    src.reserve(n.saturating_sub(src.len()));
                    return Ok(None);
                }
                let buf = src.split_to(n);
                let vs = names
                    .enumerate()
                    .filter(|(i, ..)| buf[i / 8] & (1 << (i % 8)) != 0)
                    .map(|(.., name)| name.to_string())
                    .collect();
                Ok(Some(Val::Flags(vs)))
            }
            Type::Own(ty) | Type::Borrow(ty) => {
                #[cfg(feature = "wasmtime-wasi")]
                if *ty
                    == wasmtime::component::ResourceType::host::<wasmtime_wasi::p2::DynInputStream>(
                    )
                    || self.resources.io_streams.contains(ty)
                {
                    return self.decode_async::<wasm_tokio::CoreVecDecoderBytes>(
                        src,
                        path,
                        state,
                        "stream",
                        pending_input_stream,
                        ready_input_stream,
                    );
                }
                if self.resources.guest.contains(ty) {
                    let mut id = uuid::Bytes::default();
                    if !matches!(state.take().as_deref(), Some(Partial::Uuid)) {
                        let Some(n) = Leb128DecoderU8.decode(src)? else {
                            return Ok(None);
                        };
                        if usize::from(n) != id.len() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!(
                                    "invalid guest resource handle length {n}, expected {}",
                                    id.len()
                                ),
                            ));
                        }
                    }
                    if src.len() < id.len() {
                        src.reserve(id.len().saturating_sub(src.len()));
                        *state = Some(Box::new(Partial::Uuid));
                        return Ok(None);
                    }
                    src.copy_to_slice(&mut id);
                    let id = Uuid::from_bytes_le(id);
                    trace!(?id, "lookup shared resource");
                    let resource = self.store.with_store(|mut store| {
                        store
                            .data_mut()
                            .wrpc()
                            .shared_resources
                            .0
                            .get(&id)
                            .copied()
                            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
                    })?;
                    Ok(Some(Val::Resource(resource)))
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "decoding host resources not supported yet",
                    ))
                }
            }
            Type::Future(ty) => with_payload!(
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
            ),
            Type::Stream(ty) => {
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
            Type::ErrorContext => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`error-context` not supported",
            )),
            Type::Map(..) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "maps not supported",
            )),
            Type::FixedLengthList(..) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "fixed-length lists not supported",
            )),
        }
    }
}

impl<T> Decoder for ValDecoder<'_, T>
where
    T: WithStore<Data: WrpcView>,
{
    type Item = Vec<Val>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        let tys = self.tys;
        let mut state = self.state.take();
        let mut path = Vec::default();
        let vs = self
            .decode_seq(src, &mut path, &mut state, Some(tys.len()), tys)
            .map_err(|err| {
                let kind = err.kind();
                std::io::Error::new(
                    kind,
                    wasmtime::Error::from(err)
                        .context(format!("failed to decode value at path {path:?}")),
                )
            })?;
        self.state = state;
        Ok(vs)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        if let Some(vs) = self.decode(src)? {
            return Ok(Some(vs));
        }
        if self.state.is_some() || !src.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "parameter values incomplete",
            ));
        }
        Ok(None)
    }
}
