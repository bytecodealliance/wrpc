use core::any::Any;
use core::borrow::Borrow;
use core::convert::Infallible;
use core::iter::{self, zip};
use core::mem;
use core::ops::{BitOrAssign, Bound, Shl};
use core::task::{Context, Poll, ready};

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use bytes::{Buf as _, BufMut as _, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{error, instrument, trace, warn};
use uuid::Uuid;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecEncoderBytes, DEFAULT_MAX_INITIAL_CAPACITY,
    Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64, Leb128DecoderU8, Leb128DecoderU16,
    Leb128DecoderU32, Leb128DecoderU64, Leb128Encoder, Utf8Codec,
};
use wasmtime::bail;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{Type, Val};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};
use wrpc_transport::{Receive, Transmit, error_unknown_path};

use crate::{ResourceTypes, WrpcView};

#[cfg(feature = "async")]
mod concurrent;

/// Default `max_len` of [`ValDecoder`] and [`TupleDecoder`]
pub const DEFAULT_MAX_LEN: u32 = 1 << 24;

/// Synchronous access to a store, which is either borrowed exclusively or reachable through an
/// `Accessor`.
///
/// Wasmtime only provides an `Accessor` to host functions implementing `async func` imports,
/// host functions implementing other imports hold a [`StoreContextMut`] across `await` points.
///
/// `()` provides no store, so values requiring one, such as resources, futures and streams, fail
/// to encode and decode.
pub trait WithStore {
    type Data: 'static;

    fn with_store<R>(
        &mut self,
        f: impl FnOnce(StoreContextMut<'_, Self::Data>) -> R,
    ) -> std::io::Result<R>;
}

impl<T: 'static> WithStore for StoreContextMut<'_, T> {
    type Data = T;

    fn with_store<R>(&mut self, f: impl FnOnce(StoreContextMut<'_, T>) -> R) -> std::io::Result<R> {
        Ok(f(self.as_context_mut()))
    }
}

impl WithStore for () {
    type Data = Infallible;

    fn with_store<R>(
        &mut self,
        _: impl FnOnce(StoreContextMut<'_, Infallible>) -> R,
    ) -> std::io::Result<R> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "value requires a store",
        ))
    }
}

pub struct ValEncoder<T> {
    pub store: T,
    pub ty: Type,
    pub resources: ResourceTypes,
    pub nested: Option<Box<dyn Transmit + Send>>,
}

impl<T> ValEncoder<T> {
    #[must_use]
    pub fn new(store: T, ty: Type, resources: ResourceTypes) -> Self {
        Self {
            store,
            ty,
            resources,
            nested: None,
        }
    }
}

impl<T> Transmit for ValEncoder<T> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let Some(nested) = &mut self.nested else {
            return Poll::Ready(Ok(()));
        };
        nested.poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        self.nested.as_ref().is_none_or(|nested| nested.is_closed())
    }
}

/// Encodes a tuple of [`Val`]s of types `tys`, e.g. function results.
pub struct TupleEncoder<T> {
    store: T,
    pub(crate) tys: Arc<[Type]>,
    resources: ResourceTypes,
    nested: BTreeMap<usize, Box<dyn Transmit + Send>>,
}

impl<T> TupleEncoder<T> {
    #[must_use]
    pub fn new(store: T, tys: Arc<[Type]>, resources: ResourceTypes) -> Self {
        Self {
            store,
            tys,
            resources,
            nested: BTreeMap::default(),
        }
    }
}

impl<T> Encoder<&[Val]> for TupleEncoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    type Error = std::io::Error;

    fn encode(&mut self, vs: &[Val], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode_vals(vs, dst)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
    }
}

impl<T> Encoder<Vec<Val>> for TupleEncoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    type Error = std::io::Error;

    fn encode(&mut self, vs: Vec<Val>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(vs.as_slice(), dst)
    }
}

impl<T> TupleEncoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    fn encode_vals(&mut self, vs: &[Val], dst: &mut BytesMut) -> wasmtime::Result<()> {
        if vs.len() != self.tys.len() {
            bail!("value count mismatch");
        }
        for (i, (v, ty)) in zip(vs, &*self.tys).enumerate() {
            if let Some(t) = encode_val(&mut self.store, &self.resources, ty, v, dst)
                .with_context(|| format!("failed to encode value {i}"))?
            {
                self.nested.insert(i, t);
            }
        }
        Ok(())
    }
}

impl<T> Transmit for TupleEncoder<T> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.nested.poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        self.nested.is_closed()
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

#[cfg(feature = "wasmtime-wasi")]
fn pending_input_stream<T: WrpcView>(
    store: StoreContextMut<'_, T>,
) -> wasmtime::Result<(Val, Box<dyn Receive + Send>)> {
    let (r, rx) = wrpc_transport::StreamDecoderBytes::pending();
    let v = push_input_stream(
        store,
        Box::new(wasmtime_wasi::p2::pipe::AsyncReadStream::new(
            tokio_util::io::StreamReader::new(futures::StreamExt::map(rx, Ok::<_, std::io::Error>)),
        )),
    )?;
    Ok((v, Box::new(r)))
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

impl<T> Encoder<&Val> for ValEncoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    type Error = std::io::Error;

    fn encode(&mut self, v: &Val, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let nested = encode_val(&mut self.store, &self.resources, &self.ty, v, dst)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        if let Some(t) = nested {
            self.nested = Some(t);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[instrument(level = "trace", skip(store, resources, ty))]
fn encode_val(
    store: &mut impl WithStore<Data: WrpcView>,
    resources: &ResourceTypes,
    ty: &Type,
    v: &Val,
    dst: &mut BytesMut,
) -> wasmtime::Result<Option<Box<dyn Transmit + Send>>> {
    match (v, ty) {
        (Val::Bool(v), Type::Bool) => {
            dst.reserve(1);
            dst.put_u8((*v).into());
            Ok(None)
        }
        (Val::S8(v), Type::S8) => {
            dst.reserve(1);
            dst.put_i8(*v);
            Ok(None)
        }
        (Val::U8(v), Type::U8) => {
            dst.reserve(1);
            dst.put_u8(*v);
            Ok(None)
        }
        (Val::S16(v), Type::S16) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode s16")
            .map(|()| None),
        (Val::U16(v), Type::U16) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode u16")
            .map(|()| None),
        (Val::S32(v), Type::S32) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode s32")
            .map(|()| None),
        (Val::U32(v), Type::U32) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode u32")
            .map(|()| None),
        (Val::S64(v), Type::S64) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode s64")
            .map(|()| None),
        (Val::U64(v), Type::U64) => Leb128Encoder
            .encode(*v, dst)
            .context("failed to encode u64")
            .map(|()| None),
        (Val::Float32(v), Type::Float32) => F32Codec
            .encode(*v, dst)
            .context("failed to encode f32")
            .map(|()| None),
        (Val::Float64(v), Type::Float64) => F64Codec
            .encode(*v, dst)
            .context("failed to encode f64")
            .map(|()| None),
        (Val::Char(v), Type::Char) => Utf8Codec
            .encode(*v, dst)
            .context("failed to encode char")
            .map(|()| None),
        (Val::String(v), Type::String) => CoreNameEncoder
            .encode(v.as_str(), dst)
            .context("failed to encode string")
            .map(|()| None),
        (Val::List(vs), Type::List(ty)) => {
            let ty = ty.ty();
            let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
            dst.reserve(5 + vs.len());
            Leb128Encoder
                .encode(n, dst)
                .context("failed to encode list length")?;
            let mut nested = BTreeMap::<usize, Box<dyn Transmit + Send>>::default();
            for (i, v) in vs.iter().enumerate() {
                if let Some(t) = encode_val(store, resources, &ty, v, dst)
                    .context("failed to encode list element")?
                {
                    nested.insert(i, t);
                }
            }
            if nested.is_empty() {
                return Ok(None);
            }
            Ok(Some(Box::new(nested)))
        }
        (Val::Record(vs), Type::Record(ty)) => {
            if vs.len() != ty.fields().len() {
                bail!(
                    "record has {} fields, expected {}",
                    vs.len(),
                    ty.fields().len()
                );
            }
            for ((name, ..), Field { name: field, .. }) in zip(vs, ty.fields()) {
                if name != field {
                    bail!("record field `{name}` does not match `{field}`");
                }
            }
            dst.reserve(vs.len());
            let mut nested = BTreeMap::<usize, Box<dyn Transmit + Send>>::default();
            for (i, ((name, v), Field { ref ty, .. })) in zip(vs, ty.fields()).enumerate() {
                if let Some(t) = encode_val(store, resources, ty, v, dst)
                    .with_context(|| format!("failed to encode `{name}` field"))?
                {
                    nested.insert(i, t);
                }
            }
            if nested.is_empty() {
                return Ok(None);
            }
            Ok(Some(Box::new(nested)))
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
            let mut nested = BTreeMap::<usize, Box<dyn Transmit + Send>>::default();
            for (i, (v, ref ty)) in zip(vs, ty.types()).enumerate() {
                if let Some(t) = encode_val(store, resources, ty, v, dst)
                    .context("failed to encode tuple element")?
                {
                    nested.insert(i, t);
                }
            }
            if nested.is_empty() {
                return Ok(None);
            }
            Ok(Some(Box::new(nested)))
        }
        (Val::Variant(discriminant, v), Type::Variant(ty)) => {
            let cases = ty.cases();
            let ty = match cases.len() {
                ..=0x0000_00ff => {
                    let (discriminant, ty) = find_variant_discriminant(0u8.., cases, discriminant)?;
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
                encode_val(store, resources, &ty, v, dst).context("failed to encode variant value")
            } else if ty.is_some() {
                bail!("payload missing for variant `{discriminant}`")
            } else {
                Ok(None)
            }
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
            Ok(None)
        }
        (Val::Option(None), Type::Option(_)) => {
            dst.reserve(1);
            dst.put_u8(0);
            Ok(None)
        }
        (Val::Option(Some(v)), Type::Option(ty)) => {
            dst.reserve(2);
            dst.put_u8(1);
            let ty = ty.ty();
            encode_val(store, resources, &ty, v, dst)
                .context("failed to encode `option::some` value")
        }
        (Val::Result(v), Type::Result(ty)) => match v {
            Ok(v) => match (v, ty.ok()) {
                (Some(v), Some(ty)) => {
                    dst.reserve(2);
                    dst.put_u8(0);
                    encode_val(store, resources, &ty, v, dst)
                        .context("failed to encode `result::ok` value")
                }
                (Some(_v), None) => bail!("`result::ok` value of unknown type"),
                (None, Some(_ty)) => bail!("`result::ok` value missing"),
                (None, None) => {
                    dst.reserve(1);
                    dst.put_u8(0);
                    Ok(None)
                }
            },
            Err(v) => match (v, ty.err()) {
                (Some(v), Some(ty)) => {
                    dst.reserve(2);
                    dst.put_u8(1);
                    encode_val(store, resources, &ty, v, dst)
                        .context("failed to encode `result::err` value")
                }
                (Some(_v), None) => bail!("`result::err` value of unknown type"),
                (None, Some(_ty)) => bail!("`result::err` value missing"),
                (None, None) => {
                    dst.reserve(1);
                    dst.put_u8(1);
                    Ok(None)
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
            Ok(None)
        }
        (Val::Resource(resource), Type::Own(ty) | Type::Borrow(ty)) => {
            #[cfg(feature = "wasmtime-wasi")]
            if *ty == wasmtime::component::ResourceType::host::<wasmtime_wasi::p2::DynInputStream>()
                || resources.io_streams.contains(ty)
            {
                let stream = store.with_store(|mut store| {
                    let stream = resource
                        .try_into_resource::<wasmtime_wasi::p2::DynInputStream>(&mut store)
                        .context("failed to downcast `wasi:io/input-stream`")?;
                    if !stream.owned() {
                        store
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
                    store
                        .data_mut()
                        .wrpc()
                        .table
                        .delete(stream)
                        .context("failed to delete input stream")
                })??;
                let items: core::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> =
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
                    { wrpc_transport::READ_CHUNK_SIZE },
                >::default();
                enc.encode(items, dst)
                    .context("failed to encode input stream")?;
                if enc.is_closed() {
                    return Ok(None);
                }
                return Ok(Some(Box::new(enc)));
            }
            if resources.guest.contains(ty) {
                let id = Uuid::now_v7();
                CoreVecEncoderBytes
                    .encode(id.to_bytes_le().as_slice(), dst)
                    .context("failed to encode resource handle")?;
                trace!(?id, "store shared resource");
                let prev = store.with_store(|mut store| {
                    store
                        .data_mut()
                        .wrpc()
                        .shared_resources
                        .0
                        .insert(id, *resource)
                })?;
                if prev.is_some() {
                    error!(?id, "duplicate resource ID generated");
                }
                Ok(None)
            } else {
                bail!("encoding host resources not supported yet")
            }
        }

        #[cfg(feature = "async")]
        (Val::Future(future), Type::Future(ty)) => {
            let nested =
                store.with_store(|store| concurrent::encode_future(store, future, ty))??;
            dst.reserve(1);
            dst.put_u8(0);
            Ok(Some(nested))
        }
        #[cfg(feature = "async")]
        (Val::Stream(stream), Type::Stream(ty)) => {
            let nested =
                store.with_store(|store| concurrent::encode_stream(store, stream, ty))??;
            dst.reserve(1);
            dst.put_u8(0);
            Ok(Some(nested))
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

/// Progress of a [`ValDecoder`] or [`TupleDecoder`] within a partially-received value
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

struct Inner<T> {
    store: T,
    resources: ResourceTypes,
    max_len: u32,
    state: Option<Box<Partial>>,
    /// Decoders of the pending asynchronous values keyed by their paths
    nested: BTreeMap<Box<[usize]>, Box<dyn Receive + Send>>,
}

impl<T> Inner<T> {
    fn new(store: T, resources: ResourceTypes) -> Self {
        Self {
            store,
            resources,
            max_len: DEFAULT_MAX_LEN,
            state: None,
            nested: BTreeMap::default(),
        }
    }
}

impl<T> Receive for Inner<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let Some((p, r)) = self
            .nested
            .range_mut::<[usize], _>((Bound::Unbounded, Bound::Included(path)))
            .next_back()
            .filter(|(p, ..)| path.starts_with(p))
        else {
            return Poll::Ready(Err(error_unknown_path(path)));
        };
        ready!(r.poll_receive(cx, &path[p.len()..], src))?;
        if r.is_closed() {
            let p = p.clone();
            self.nested.remove(&p);
        }
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        self.nested.is_empty()
    }
}

/// Decodes a [`Val`] of type `ty`.
///
/// Resources and asynchronous values are created in the store once their encoding is complete.
/// Pending asynchronous values are received using [`Receive::poll_receive`].
pub struct ValDecoder<T> {
    ty: Type,
    inner: Inner<T>,
}

impl<T> ValDecoder<T> {
    #[must_use]
    pub fn new(store: T, ty: Type, resources: ResourceTypes) -> Self {
        Self {
            ty,
            inner: Inner::new(store, resources),
        }
    }

    /// Rejects strings and ready `input-stream`s longer than `max_len` bytes and lists longer than
    /// `max_len` elements, [`DEFAULT_MAX_LEN`] by default.
    #[must_use]
    pub fn with_max_len(mut self, max_len: u32) -> Self {
        self.inner.max_len = max_len;
        self
    }
}

impl<T> Receive for ValDecoder<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.inner.poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

/// Decodes a tuple of [`Val`]s of types `tys`, e.g. function parameters.
///
/// Resources and asynchronous values are created in the store once their encoding is complete.
/// Pending asynchronous values are received using [`Receive::poll_receive`].
pub struct TupleDecoder<T> {
    tys: Arc<[Type]>,
    inner: Inner<T>,
}

impl<T> TupleDecoder<T> {
    #[must_use]
    pub fn new(store: T, tys: Arc<[Type]>, resources: ResourceTypes) -> Self {
        Self {
            tys,
            inner: Inner::new(store, resources),
        }
    }

    /// Rejects strings and ready `input-stream`s longer than `max_len` bytes and lists longer than
    /// `max_len` elements, [`DEFAULT_MAX_LEN`] by default.
    #[must_use]
    pub fn with_max_len(mut self, max_len: u32) -> Self {
        self.inner.max_len = max_len;
        self
    }
}

impl<T> Receive for TupleDecoder<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.inner.poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
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
    dec: T,
) -> std::io::Result<Option<T::Item>>
where
    T: Decoder + Send + 'static,
    std::io::Error: From<T::Error>,
{
    let mut dec = match state.take().map(|state| *state) {
        Some(Partial::Decoder(prev)) => prev.downcast().map_or(dec, |prev| *prev),
        _ => dec,
    };
    let v = dec.decode(src)?;
    if v.is_none() {
        *state = Some(Box::new(Partial::Decoder(Box::new(dec))));
    }
    Ok(v)
}

impl<T> Inner<T>
where
    T: WithStore<Data: WrpcView>,
{
    fn decode<V>(
        &mut self,
        src: &mut BytesMut,
        f: impl FnOnce(
            &mut Self,
            &mut BytesMut,
            &mut Vec<usize>,
            &mut Option<Box<Partial>>,
        ) -> std::io::Result<Option<V>>,
    ) -> std::io::Result<Option<V>> {
        let n = self.state.is_none().then_some(src.len());
        let mut state = self.state.take();
        let mut path = Vec::default();
        let v = f(self, src, &mut path, &mut state).map_err(|err| {
            let kind = err.kind();
            std::io::Error::new(
                kind,
                wasmtime::Error::from(err)
                    .context(format!("failed to decode value at path {path:?}")),
            )
        })?;
        if n != Some(src.len()) {
            self.state = state;
        }
        Ok(v)
    }

    fn decode_eof<V>(&self, v: Option<V>, src: &BytesMut) -> std::io::Result<Option<V>> {
        if v.is_none() && (self.state.is_some() || !src.is_empty()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "value incomplete",
            ));
        }
        Ok(v)
    }

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
                    if n > self.max_len {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "list length `{n}` exceeds the maximum of `{}`",
                                self.max_len
                            ),
                        ));
                    }
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

    #[allow(clippy::too_many_arguments)]
    #[cfg(any(feature = "async", feature = "wasmtime-wasi"))]
    fn decode_async<D>(
        &mut self,
        src: &mut BytesMut,
        path: &[usize],
        state: &mut Option<Box<Partial>>,
        kind: &str,
        dec: D,
        pending: impl FnOnce(
            StoreContextMut<'_, T::Data>,
        ) -> wasmtime::Result<(Val, Box<dyn Receive + Send>)>,
        ready: impl FnOnce(StoreContextMut<'_, T::Data>, D::Item) -> wasmtime::Result<Val>,
    ) -> std::io::Result<Option<Val>>
    where
        D: Decoder + Send + 'static,
        std::io::Error: From<D::Error>,
    {
        if state.is_none() {
            let Some(status) = decode_status(src, kind)? else {
                return Ok(None);
            };
            if status == 0 {
                let res = self.store.with_store(pending)?;
                let (v, r) = res.map_err(std::io::Error::other)?;
                self.nested.insert(path.into(), r);
                return Ok(Some(v));
            }
        }
        let Some(v) = decode_with(src, state, dec)? else {
            return Ok(None);
        };
        let v = self.store.with_store(|store| ready(store, v))?;
        let v = v.map_err(std::io::Error::other)?;
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
                let v = decode_with(src, state, <CoreNameDecoder>::with_max_len(self.max_len))?;
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
                    return self.decode_async(
                        src,
                        path,
                        state,
                        "stream",
                        <wasm_tokio::CoreVecDecoderBytes>::with_max_len(self.max_len),
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
                    })??;
                    Ok(Some(Val::Resource(resource)))
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "decoding host resources not supported yet",
                    ))
                }
            }
            #[cfg(feature = "async")]
            Type::Future(ty) => self.decode_future(src, path, state, ty),
            #[cfg(feature = "async")]
            Type::Stream(ty) => self.decode_stream(src, path, state, ty),
            #[cfg(not(feature = "async"))]
            Type::Future(..) | Type::Stream(..) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`future` and `stream` require the `async` feature",
            )),
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

impl<T> Decoder for ValDecoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    type Item = Val;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Val>> {
        let ty = &self.ty;
        self.inner.decode(src, |inner, src, path, state| {
            inner.decode_ty(src, ty, path, state)
        })
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Val>> {
        let v = self.decode(src)?;
        self.inner.decode_eof(v, src)
    }
}

impl<T> Decoder for TupleDecoder<T>
where
    T: WithStore<Data: WrpcView>,
{
    type Item = Vec<Val>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        let tys = &*self.tys;
        self.inner.decode(src, |inner, src, path, state| {
            inner.decode_seq(src, path, state, Some(tys.len()), tys)
        })
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        let vs = self.decode(src)?;
        self.inner.decode_eof(vs, src)
    }
}
