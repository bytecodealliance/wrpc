use core::future::{pending, ready};
use core::iter::zip;
use core::mem;
use core::ops::{BitOrAssign, Shl};
use core::pin::Pin;
use core::task::{Context, Poll};

use std::collections::HashSet;

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::stream::{self, FuturesUnordered};
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::io::AsyncReadExt as _;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Decoder, Encoder, FramedRead};
use tokio_util::sync::PollSender;
use tracing::{error, instrument, trace, warn};
use uuid::Uuid;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64,
    Leb128DecoderU8, Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Leb128Encoder,
    Utf8Codec,
};
use wasmtime::bail;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{
    Accessor, ComponentType, Destination, FutureAny, FutureConsumer, FutureReader, Lift, Lower,
    ResourceType, Source, StreamAny, StreamConsumer, StreamProducer, StreamReader, StreamResult,
    Type, Val, VecBuffer,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};
use wrpc_transport::frame::{Incoming, Outgoing};
use wrpc_transport::{Decode, Encode, READ_CHUNK_SIZE, Sink as _};

use crate::{RemoteResource, ResourceTypes, WrpcView};

type DeferredFn =
    Box<dyn FnOnce(Outgoing) -> Pin<Box<dyn Future<Output = wasmtime::Result<()>> + Send>> + Send>;

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
    pub deferred: Option<DeferredFn>,
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

async fn write_deferred<I>(w: Outgoing, deferred: I) -> wasmtime::Result<()>
where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator<Item = Option<DeferredFn>>,
{
    let mut futs: FuturesUnordered<_> = zip(0.., deferred)
        .filter_map(|(i, f)| f.map(|f| (w.index(&[i]), f)))
        .map(|(w, f)| async move {
            let w = w.map_err(wasmtime::Error::from)?;
            f(w).await
        })
        .collect();
    while let Some(()) = futs.try_next().await? {}
    Ok(())
}

trait Payload: ComponentType + Lift + Lower + Send + Sync + Unpin + 'static {
    fn from_val(v: Option<Val>) -> Option<Self>;
}

impl Payload for () {
    fn from_val(v: Option<Val>) -> Option<Self> {
        v.is_none().then_some(())
    }
}

macro_rules! impl_payload {
    ($($t:ty => $case:ident),+ $(,)?) => {
        $(
            impl Payload for $t {
                fn from_val(v: Option<Val>) -> Option<Self> {
                    if let Some(Val::$case(v)) = v {
                        Some(v)
                    } else {
                        None
                    }
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
) -> wasmtime::Result<DeferredFn>
where
    P: Payload + Encode,
    std::io::Error: From<<P::Encoder as Encoder<P>>::Error>,
{
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

    let (tx, rx) = oneshot::channel();
    let future = future.try_into_future_reader::<P>()?;
    future.pipe(store, Consumer(Some(tx)))?;
    Ok(Box::new(move |w| {
        Box::pin(async move {
            let v = rx.await.context("future dropped without a value")?;
            w.write_future(offset, ready(v)).await?;
            Ok(())
        })
    }))
}

fn encode_stream<P, T>(
    store: StoreContextMut<'_, T>,
    stream: StreamAny,
    offset: usize,
) -> wasmtime::Result<DeferredFn>
where
    P: Payload + Encode,
    std::io::Error: From<<P::Encoder as Encoder<P>>::Error>,
{
    struct Consumer<P: Send + 'static>(PollSender<Vec<P>>);

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
            let n = source.remaining(&mut store);
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
    Ok(Box::new(move |w| {
        Box::pin(async move {
            w.write_stream(offset, stream::poll_fn(move |cx| rx.poll_recv(cx)))
                .await?;
            Ok(())
        })
    }))
}

fn pending_future<P, D>(
    mut store: StoreContextMut<'_, D>,
    rx: oneshot::Receiver<Incoming>,
) -> wasmtime::Result<FutureAny>
where
    P: Payload + Decode,
    D: 'static,
    std::io::Error: From<<P::Decoder as Decoder>::Error>,
{
    let fut = FutureReader::new(&mut store, async move {
        let Ok(r) = rx.await else {
            error!("future I/O dropped");
            return pending().await;
        };
        let mut rx = FramedRead::new(r, P::Decoder::default());
        let v = rx.next().await.map(|v| v.map_err(std::io::Error::from));
        let processed = rx.read_buffer().is_empty();
        drop(rx);
        match v {
            Some(Ok(v)) if processed => wasmtime::error::Ok(v),
            Some(Ok(..)) => {
                error!("received data after future value");
                pending().await
            }
            Some(Err(err)) => {
                error!(%err, "failed to receive future value");
                pending().await
            }
            None => {
                error!("future value missing");
                pending().await
            }
        }
    })?;
    fut.try_into_future_any(store)
}

fn ready_future<P, D>(mut store: StoreContextMut<'_, D>, v: P) -> wasmtime::Result<FutureAny>
where
    P: Payload,
    D: 'static,
{
    let fut = FutureReader::new(&mut store, ready(wasmtime::error::Ok(v)))?;
    fut.try_into_future_any(store)
}

struct Producer<P: Decode> {
    pending: Option<oneshot::Receiver<Incoming>>,
    rx: Option<FramedRead<Incoming, P::ListDecoder>>,
}

impl<P, D> StreamProducer<D> for Producer<P>
where
    P: Payload + Decode<ListDecoder: Send>,
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
        if dst.remaining(store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if let Some(pending) = &mut self.pending {
            match Pin::new(pending).poll(cx) {
                Poll::Ready(Ok(r)) => {
                    self.pending = None;
                    self.rx = Some(FramedRead::new(r, P::ListDecoder::default()));
                }
                Poll::Ready(Err(..)) => {
                    error!("stream I/O dropped");
                    self.pending = None;
                }
                Poll::Pending => {}
            }
        }
        if let Some(rx) = &mut self.rx {
            match rx.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() && rx.read_buffer().is_empty() => {
                    self.rx = None;
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => {
                    error!("received data after stream end");
                    self.rx = None;
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    dst.set_buffer(chunk.into());
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                Poll::Ready(Some(Err(err))) => {
                    error!(err = %std::io::Error::from(err), "failed to receive stream chunk");
                    self.rx = None;
                }
                Poll::Ready(None) => {
                    error!("stream end missing");
                    self.rx = None;
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
    rx: oneshot::Receiver<Incoming>,
) -> wasmtime::Result<StreamAny>
where
    P: Payload + Decode<ListDecoder: Send>,
    D: 'static,
    std::io::Error: From<<P::ListDecoder as Decoder>::Error>,
{
    let stream = StreamReader::new(
        &mut store,
        Producer::<P> {
            pending: Some(rx),
            rx: None,
        },
    )?;
    stream.try_into_stream_any(store)
}

fn ready_stream<P, D>(mut store: StoreContextMut<'_, D>, vs: Vec<P>) -> wasmtime::Result<StreamAny>
where
    P: Payload,
    D: 'static,
{
    let stream = StreamReader::new(&mut store, vs)?;
    stream.try_into_stream_any(store)
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
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
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
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
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
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
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
                    if let Some(f) = enc.deferred {
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
                if let Some(f) = enc.deferred {
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
                        if let Some(f) = enc.deferred {
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
                        if let Some(f) = enc.deferred {
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
                if *ty == ResourceType::host::<wasmtime_wasi::p2::DynInputStream>()
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
                        if let Some(f) = wrpc_transport::Deferred::take_deferred(&mut enc) {
                            self.deferred = Some(Box::new(|w| {
                                Box::pin(async move { Ok(f(w, Vec::default()).await?) })
                            }));
                        }
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
                if resource.ty() == ResourceType::host::<RemoteResource>() {
                    let resource = resource
                        .try_into_resource(&mut self.store)
                        .context("resource type mismatch")?;
                    let table = self.store.data_mut().wrpc().table;
                    if resource.owned() {
                        let RemoteResource(buf) = table
                            .delete(resource)
                            .context("failed to delete remote resource")?;
                        CoreVecEncoderBytes
                            .encode(buf, dst)
                            .context("failed to encode resource handle")
                    } else {
                        let RemoteResource(buf) = table
                            .get(&resource)
                            .context("failed to get remote resource")?;
                        CoreVecEncoderBytes
                            .encode(buf, dst)
                            .context("failed to encode resource handle")
                    }
                } else if self.resources.guest.contains(ty) {
                    let id = Uuid::now_v7();
                    CoreVecEncoderBytes
                        .encode(id.to_bytes_le().as_slice(), dst)
                        .context("failed to encode resource handle")?;
                    trace!(?id, "store shared resource");
                    if self
                        .store
                        .data_mut()
                        .wrpc()
                        .ctx
                        .shared_resources()
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

/// Hands the sub-streams of an [`Incoming`] over to the asynchronous values of a decoded [`Val`]
pub type IncomingDeferredFn = Box<dyn FnOnce(&Incoming) -> std::io::Result<()> + Send>;

/// Progress of a [`ValDecoder`] within a partially-received value
enum Partial {
    Bytes(CoreVecDecoderBytes<DEFAULT_MAX_INITIAL_CAPACITY>),
    Name(CoreNameDecoder<DEFAULT_MAX_INITIAL_CAPACITY>),
    Uuid,
    Seq {
        n: usize,
        vs: Vec<Val>,
        elem: Option<Box<Partial>>,
    },
    Case {
        status: usize,
        payload: Option<Box<Partial>>,
    },
}

/// Decodes a [`Val`] of [`Type`].
///
/// Resources and asynchronous values are created in the store once their encoding is complete,
/// asynchronous values are fed from the sub-streams of an [`Incoming`] handed to them by
/// [`ValDecoder::take_deferred`], which must be run after each call to [`Decoder::decode`],
/// since the transport may wait for sub-streams to be indexed before receiving more data.
pub struct ValDecoder<'a, S> {
    store: &'a mut S,
    ty: &'a Type,
    resources: &'a ResourceTypes,
    path: &'a [usize],
    state: Option<Box<Partial>>,
    deferred: Vec<(Box<[usize]>, oneshot::Sender<Incoming>)>,
    scratch: Vec<usize>,
}

impl<S> ValDecoder<'_, S> {
    #[must_use]
    pub fn new<'a>(
        store: &'a mut S,
        ty: &'a Type,
        resources: &'a ResourceTypes,
        path: &'a [usize],
    ) -> ValDecoder<'a, S> {
        ValDecoder {
            store,
            ty,
            resources,
            path,
            state: None,
            deferred: Vec::default(),
            scratch: Vec::default(),
        }
    }

    /// Takes the operation handing sub-streams of an [`Incoming`] over to the asynchronous
    /// values decoded since the last call, if any
    pub fn take_deferred(&mut self) -> Option<IncomingDeferredFn> {
        if self.deferred.is_empty() {
            return None;
        }
        let deferred = mem::take(&mut self.deferred);
        Some(Box::new(move |r| {
            for (path, tx) in deferred {
                let r = r.index(&path).map_err(std::io::Error::other)?;
                _ = tx.send(r);
            }
            Ok(())
        }))
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

fn decode_bytes(
    src: &mut BytesMut,
    state: &mut Option<Box<Partial>>,
) -> std::io::Result<Option<Bytes>> {
    let mut dec = match state.take().map(|state| *state) {
        Some(Partial::Bytes(dec)) => dec,
        _ => CoreVecDecoderBytes::default(),
    };
    let v = dec.decode(src)?;
    if v.is_none() {
        *state = Some(Box::new(Partial::Bytes(dec)));
    }
    Ok(v)
}

fn decode_discriminant(src: &mut BytesMut) -> std::io::Result<Option<usize>> {
    let Some(v) = Leb128DecoderU32.decode(src)? else {
        return Ok(None);
    };
    let v = usize::try_from(v)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    Ok(Some(v))
}

fn decode_leaf<D>(
    mut dec: D,
    src: &mut BytesMut,
    f: impl FnOnce(D::Item) -> Val,
) -> std::io::Result<Option<Val>>
where
    D: Decoder<Error = std::io::Error>,
{
    let v = dec.decode(src)?;
    Ok(v.map(f))
}

impl<S> ValDecoder<'_, S>
where
    S: WithStore<Data: WrpcView>,
{
    fn decode_seq(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
        len: Option<usize>,
        mut ty: impl FnMut(usize) -> Option<Type>,
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
        while vs.len() < n {
            let i = vs.len();
            let elem_ty = ty(i).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "element type missing")
            })?;
            path.push(i);
            let v = self.decode_ty(src, &elem_ty, path, &mut elem);
            path.pop();
            let Some(v) = v? else {
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
        status: impl FnOnce(&mut BytesMut) -> std::io::Result<Option<usize>>,
        ty: impl FnOnce(usize) -> std::io::Result<Option<Type>>,
    ) -> std::io::Result<Option<(usize, Option<Val>)>> {
        let (status, mut payload) = match state.take().map(|state| *state) {
            Some(Partial::Case { status, payload }) => (status, payload),
            _ => {
                let Some(status) = status(src)? else {
                    return Ok(None);
                };
                (status, None)
            }
        };
        let Some(ty) = ty(status)? else {
            return Ok(Some((status, None)));
        };
        let Some(v) = self.decode_ty(src, &ty, path, &mut payload)? else {
            *state = Some(Box::new(Partial::Case { status, payload }));
            return Ok(None);
        };
        Ok(Some((status, Some(v))))
    }

    fn subscribe(&mut self, path: &[usize]) -> oneshot::Receiver<Incoming> {
        let (tx, rx) = oneshot::channel();
        self.deferred.push((Box::from(path), tx));
        rx
    }

    fn decode_future<P>(
        &mut self,
        src: &mut BytesMut,
        ty: Option<&Type>,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
    ) -> std::io::Result<Option<Val>>
    where
        P: Payload + Decode,
        std::io::Error: From<<P::Decoder as Decoder>::Error>,
    {
        let Some((status, v)) = self.decode_case(
            src,
            path,
            state,
            |src| decode_status(src, "future"),
            |status| Ok(if status == 1 { ty.cloned() } else { None }),
        )?
        else {
            return Ok(None);
        };
        let future = if status == 0 {
            let rx = self.subscribe(path);
            self.store
                .with_store(|store| pending_future::<P, _>(store, rx))
        } else {
            let v = P::from_val(v)
                .ok_or_else(|| std::io::Error::other("payload value type mismatch"))?;
            self.store.with_store(|store| ready_future(store, v))
        };
        let future = future.map_err(std::io::Error::other)?;
        Ok(Some(Val::Future(future)))
    }

    fn decode_stream<P>(
        &mut self,
        src: &mut BytesMut,
        ty: &Type,
        path: &mut Vec<usize>,
        state: &mut Option<Box<Partial>>,
    ) -> std::io::Result<Option<Val>>
    where
        P: Payload + Decode<ListDecoder: Send>,
        std::io::Error: From<<P::ListDecoder as Decoder>::Error>,
    {
        let mut elems = match state.take().map(|state| *state) {
            Some(Partial::Case { payload, .. }) => payload,
            _ => match decode_status(src, "stream")? {
                None => return Ok(None),
                Some(0) => {
                    let rx = self.subscribe(path);
                    let stream = self
                        .store
                        .with_store(|store| pending_stream::<P, _>(store, rx))
                        .map_err(std::io::Error::other)?;
                    return Ok(Some(Val::Stream(stream)));
                }
                Some(..) => None,
            },
        };
        let Some(vs) = self.decode_seq(src, path, &mut elems, None, |_| Some(ty.clone()))? else {
            *state = Some(Box::new(Partial::Case {
                status: 1,
                payload: elems,
            }));
            return Ok(None);
        };
        let mut payloads = Vec::with_capacity(vs.len());
        for v in vs {
            let v = P::from_val(Some(v))
                .ok_or_else(|| std::io::Error::other("payload value type mismatch"))?;
            payloads.push(v);
        }
        let stream = self
            .store
            .with_store(|store| ready_stream(store, payloads))
            .map_err(std::io::Error::other)?;
        Ok(Some(Val::Stream(stream)))
    }

    #[cfg(feature = "wasmtime-wasi")]
    fn decode_input_stream(
        &mut self,
        src: &mut BytesMut,
        path: &[usize],
        state: &mut Option<Box<Partial>>,
    ) -> std::io::Result<Option<Val>> {
        let mut bytes = match state.take().map(|state| *state) {
            Some(Partial::Case { payload, .. }) => payload,
            _ => match decode_status(src, "stream")? {
                None => return Ok(None),
                Some(0) => {
                    let rx = self.subscribe(path);
                    let r = stream::once(rx)
                        .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                        .map_ok(|r| {
                            stream::try_unfold(
                                FramedRead::new(
                                    r,
                                    CoreVecDecoderBytes::<DEFAULT_MAX_INITIAL_CAPACITY>::default(),
                                ),
                                |mut framed| async move {
                                    match framed.next().await {
                                        Some(Ok(chunk)) if chunk.is_empty() => {
                                            if framed.read_buffer().is_empty() {
                                                Ok(None)
                                            } else {
                                                Err(std::io::Error::new(
                                                    std::io::ErrorKind::InvalidData,
                                                    "received data after stream end",
                                                ))
                                            }
                                        }
                                        Some(Ok(chunk)) => Ok(Some((chunk, framed))),
                                        Some(Err(err)) => Err(err),
                                        None => Err(std::io::ErrorKind::UnexpectedEof.into()),
                                    }
                                },
                            )
                        })
                        .try_flatten();
                    let stream: wasmtime_wasi::p2::DynInputStream =
                        Box::new(wasmtime_wasi::p2::pipe::AsyncReadStream::new(
                            tokio_util::io::StreamReader::new(r),
                        ));
                    return self.push_input_stream(stream).map(Some);
                }
                Some(..) => None,
            },
        };
        let Some(buf) = decode_bytes(src, &mut bytes)? else {
            *state = Some(Box::new(Partial::Case {
                status: 1,
                payload: bytes,
            }));
            return Ok(None);
        };
        let stream: wasmtime_wasi::p2::DynInputStream =
            Box::new(wasmtime_wasi::p2::pipe::MemoryInputPipe::new(buf));
        self.push_input_stream(stream).map(Some)
    }

    #[cfg(feature = "wasmtime-wasi")]
    fn push_input_stream(
        &mut self,
        stream: wasmtime_wasi::p2::DynInputStream,
    ) -> std::io::Result<Val> {
        // The stream must be typed as `DynInputStream` (the host resource type),
        // otherwise the resulting resource handle carries the concrete reader type
        // and fails the guest's `own<input-stream>` type check.
        let v = self.store.with_store(|mut store| {
            let res = store
                .data_mut()
                .wrpc()
                .table
                .push(stream)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
            res.try_into_resource_any(store)
                .map_err(std::io::Error::other)
        })?;
        Ok(Val::Resource(v))
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
                let mut dec = match state.take().map(|state| *state) {
                    Some(Partial::Name(dec)) => dec,
                    _ => CoreNameDecoder::default(),
                };
                let v = dec.decode(src)?;
                if v.is_none() {
                    *state = Some(Box::new(Partial::Name(dec)));
                }
                Ok(v.map(Val::String))
            }
            Type::List(ty) => {
                let ty = ty.ty();
                let vs = self.decode_seq(src, path, state, None, |_| Some(ty.clone()))?;
                Ok(vs.map(Val::List))
            }
            Type::Record(ty) => {
                let n = ty.fields().len();
                let Some(vs) = self.decode_seq(src, path, state, Some(n), |i| {
                    ty.fields().nth(i).map(|Field { ty, .. }| ty)
                })?
                else {
                    return Ok(None);
                };
                let vs = zip(ty.fields(), vs)
                    .map(|(Field { name, .. }, v)| (name.to_string(), v))
                    .collect();
                Ok(Some(Val::Record(vs)))
            }
            Type::Tuple(ty) => {
                let n = ty.types().len();
                let vs = self.decode_seq(src, path, state, Some(n), |i| ty.types().nth(i))?;
                Ok(vs.map(Val::Tuple))
            }
            Type::Variant(ty) => {
                let case = |discriminant| {
                    ty.cases().nth(discriminant).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("unknown variant discriminant `{discriminant}`"),
                        )
                    })
                };
                let Some((discriminant, v)) =
                    self.decode_case(src, path, state, decode_discriminant, |discriminant| {
                        let Case { ty, .. } = case(discriminant)?;
                        Ok(ty)
                    })?
                else {
                    return Ok(None);
                };
                let Case { name, .. } = case(discriminant)?;
                Ok(Some(Val::Variant(name.to_string(), v.map(Box::new))))
            }
            Type::Enum(ty) => {
                let Some(discriminant) = decode_discriminant(src)? else {
                    return Ok(None);
                };
                let name = ty.names().nth(discriminant).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("unknown enum discriminant `{discriminant}`"),
                    )
                })?;
                Ok(Some(Val::Enum(name.to_string())))
            }
            Type::Option(ty) => {
                let v = self.decode_case(
                    src,
                    path,
                    state,
                    |src| decode_status(src, "option"),
                    |status| Ok((status == 1).then(|| ty.ty())),
                )?;
                Ok(v.map(|(.., v)| Val::Option(v.map(Box::new))))
            }
            Type::Result(ty) => {
                let v = self.decode_case(
                    src,
                    path,
                    state,
                    |src| decode_status(src, "result"),
                    |status| Ok(if status == 0 { ty.ok() } else { ty.err() }),
                )?;
                Ok(v.map(|(status, v)| {
                    let v = v.map(Box::new);
                    Val::Result(if status == 0 { Ok(v) } else { Err(v) })
                }))
            }
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
                if *ty == ResourceType::host::<wasmtime_wasi::p2::DynInputStream>()
                    || self.resources.io_streams.contains(ty)
                {
                    return self.decode_input_stream(src, path, state);
                }
                if self.resources.guest.contains(ty) {
                    let mut id = uuid::Bytes::default();
                    if !matches!(state.take().as_deref(), Some(Partial::Uuid)) {
                        let Some(n) = Leb128DecoderU8.decode(src)? else {
                            return Ok(None);
                        };
                        if usize::from(n) != id.len() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
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
                            .ctx
                            .shared_resources()
                            .0
                            .get(&id)
                            .copied()
                            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
                    })?;
                    Ok(Some(Val::Resource(resource)))
                } else {
                    let Some(buf) = decode_bytes(src, state)? else {
                        return Ok(None);
                    };
                    let resource = self.store.with_store(|mut store| {
                        let resource = store
                            .data_mut()
                            .wrpc()
                            .table
                            .push(RemoteResource(Bytes::copy_from_slice(&buf)))
                            .map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::OutOfMemory, err)
                            })?;
                        resource
                            .try_into_resource_any(store)
                            .map_err(std::io::Error::other)
                    })?;
                    Ok(Some(Val::Resource(resource)))
                }
            }
            Type::Future(ty) => {
                let payload = ty.ty();
                with_payload!(
                    payload,
                    P => self.decode_future::<P>(src, payload.as_ref(), path, state),
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "`future` payload type not supported, only primitive types and `string` are",
                    ))
                )
            }
            Type::Stream(ty) => {
                let Some(payload) = ty.ty() else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "`stream` without a payload type not supported",
                    ));
                };
                with_payload!(
                    @some payload,
                    P => self.decode_stream::<P>(src, &payload, path, state),
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

impl<S> Decoder for ValDecoder<'_, S>
where
    S: WithStore<Data: WrpcView>,
{
    type Item = Val;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Val>> {
        let ty = self.ty;
        let mut path = mem::take(&mut self.scratch);
        path.clear();
        path.extend_from_slice(self.path);
        let mut state = self.state.take();
        let res = self.decode_ty(src, ty, &mut path, &mut state);
        self.scratch = path;
        match res {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => {
                self.state = state;
                Ok(None)
            }
            Err(err) => {
                self.deferred.clear();
                Err(err)
            }
        }
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Val>> {
        let v = self.decode(src)?;
        if v.is_none() && (self.state.is_some() || !src.is_empty()) {
            self.state = None;
            self.deferred.clear();
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(v)
    }
}

/// Read encoded value of type [`Type`] from an [`AsyncRead`](tokio::io::AsyncRead) into a [`Val`]
#[instrument(level = "trace", skip_all, fields(ty, path))]
pub async fn read_value<T>(
    store: &mut impl WithStore<Data = T>,
    r: &mut Pin<&mut Incoming>,
    resources: &ResourceTypes,
    val: &mut Val,
    ty: &Type,
    path: &[usize],
) -> std::io::Result<()>
where
    T: WrpcView + 'static,
{
    let mut dec = ValDecoder::new(store, ty, resources, path);
    let mut buf = BytesMut::default();
    loop {
        let v = dec.decode(&mut buf)?;
        if let Some(f) = dec.take_deferred() {
            f(r)?;
        }
        if let Some(v) = v {
            *val = v;
            return Ok(());
        }
        let b = r.read_u8().await?;
        buf.put_u8(b);
    }
}
