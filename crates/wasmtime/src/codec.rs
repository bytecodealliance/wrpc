use core::any::Any;
use core::borrow::Borrow;
use core::future;
use core::iter::{self, zip};
use core::mem;
use core::ops::Bound;
use core::task::{Context, Poll, ready};

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use bytes::{Buf as _, BufMut as _, BytesMut};
use futures::{StreamExt as _, TryFutureExt as _, stream};
use tokio_util::codec::{Decoder, Encoder};
use tracing::instrument;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64,
    Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Leb128Encoder, Utf8Codec,
};
use wasmtime::component::Type;
use wasmtime::component::types::{Case, Field};
use wasmtime::error::Context as _;
use wasmtime::{bail, format_err};
use wrpc_transport::{Encode, FutureEncoder, Receive, StreamEncoder, Transmit, error_unknown_path};

use crate::Val;
use crate::codec::receive::{pending_future, pending_stream};

mod receive;

/// Default `max_len` of [`TupleDecoder`]
pub const DEFAULT_MAX_LEN: u32 = 1 << 24;

/// Encodes [`Val`]s of types `tys`, e.g. function results.
///
/// Pending asynchronous values are transmitted using [`Transmit::poll_transmit`].
pub struct TupleEncoder {
    tys: Arc<[Type]>,
    nested: BTreeMap<usize, Box<dyn Transmit + Send>>,
}

impl TupleEncoder {
    #[must_use]
    pub fn new(tys: Arc<[Type]>) -> Self {
        Self {
            tys,
            nested: BTreeMap::default(),
        }
    }
}

impl Encoder<Vec<Val>> for TupleEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, vs: Vec<Val>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if vs.len() != self.tys.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "value count mismatch",
            ));
        }
        for (i, (v, ty)) in zip(vs, &*self.tys).enumerate() {
            let t = encode_value(ty, v, dst)
                .with_context(|| format!("failed to encode value {i}"))
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            if let Some(t) = t {
                self.nested.insert(i, t);
            }
        }
        Ok(())
    }
}

impl Transmit for TupleEncoder {
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

fn encode_seq(
    tys: impl IntoIterator<Item: Borrow<Type>>,
    vs: impl IntoIterator<Item = Val>,
    dst: &mut BytesMut,
) -> wasmtime::Result<Option<Box<dyn Transmit + Send>>> {
    let mut nested = BTreeMap::<usize, Box<dyn Transmit + Send>>::default();
    for (i, (v, ty)) in zip(vs, tys).enumerate() {
        let t = encode_value(ty.borrow(), v, dst)
            .with_context(|| format!("failed to encode element {i}"))?;
        if let Some(t) = t {
            nested.insert(i, t);
        }
    }
    if nested.is_empty() {
        return Ok(None);
    }
    Ok(Some(Box::new(nested)))
}

fn encode_payload(
    ty: Option<Type>,
    v: Option<Box<Val>>,
    dst: &mut BytesMut,
) -> wasmtime::Result<Option<Box<dyn Transmit + Send>>> {
    match (v, ty) {
        (Some(v), Some(ty)) => encode_value(&ty, *v, dst).context("failed to encode payload"),
        (Some(..), None) => bail!("payload of unknown type"),
        (None, Some(..)) => bail!("payload missing"),
        (None, None) => Ok(None),
    }
}

/// Payload of a `future` or an element of a `stream` along with its type, if any
struct Payload(Option<Type>, Option<Val>);

#[derive(Default)]
struct PayloadEncoder(Option<Box<dyn Transmit + Send>>);

impl Encoder<Payload> for PayloadEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, Payload(ty, v): Payload, dst: &mut BytesMut) -> std::io::Result<()> {
        let nested = match (ty, v) {
            (Some(ty), Some(v)) => encode_value(&ty, v, dst),
            (None, None) => Ok(None),
            _ => Err(format_err!("payload type mismatch")),
        };
        self.0 =
            nested.map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        Ok(())
    }
}

impl Transmit for PayloadEncoder {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        match &mut self.0 {
            Some(nested) => nested.poll_transmit(cx, path, dst),
            None => Poll::Ready(Ok(())),
        }
    }

    fn is_closed(&self) -> bool {
        self.0.as_ref().is_none_or(Transmit::is_closed)
    }
}

impl Encode for Payload {
    type Encoder = PayloadEncoder;
}

#[allow(clippy::too_many_lines)]
#[instrument(level = "trace", skip(ty))]
fn encode_value(
    ty: &Type,
    v: Val,
    dst: &mut BytesMut,
) -> wasmtime::Result<Option<Box<dyn Transmit + Send>>> {
    match (v, ty) {
        (Val::Bool(v), Type::Bool) => {
            dst.put_u8(v.into());
            Ok(None)
        }
        (Val::S8(v), Type::S8) => {
            dst.put_i8(v);
            Ok(None)
        }
        (Val::U8(v), Type::U8) => {
            dst.put_u8(v);
            Ok(None)
        }
        (Val::S16(v), Type::S16) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::U16(v), Type::U16) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::S32(v), Type::S32) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::U32(v), Type::U32) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::S64(v), Type::S64) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::U64(v), Type::U64) => {
            Leb128Encoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::Float32(v), Type::Float32) => {
            F32Codec.encode(v, dst)?;
            Ok(None)
        }
        (Val::Float64(v), Type::Float64) => {
            F64Codec.encode(v, dst)?;
            Ok(None)
        }
        (Val::Char(v), Type::Char) => {
            Utf8Codec.encode(v, dst)?;
            Ok(None)
        }
        (Val::String(v), Type::String) => {
            CoreNameEncoder.encode(v, dst)?;
            Ok(None)
        }
        (Val::List(vs), Type::List(ty)) => {
            let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
            Leb128Encoder.encode(n, dst)?;
            encode_seq(iter::repeat(ty.ty()), vs, dst)
        }
        (Val::Map(vs), Type::Map(ty)) => {
            let n = u32::try_from(vs.len()).context("map length does not fit in u32")?;
            Leb128Encoder.encode(n, dst)?;
            let (key, value) = (ty.key(), ty.value());
            let mut keys = HashSet::<Vec<u8>>::default();
            let mut nested = BTreeMap::<usize, Box<dyn Transmit + Send>>::default();
            for (i, (k, v)) in vs.into_iter().enumerate() {
                let off = dst.len();
                let k = encode_value(&key, k, dst)
                    .with_context(|| format!("failed to encode key {i}"))?;
                if !keys.insert(dst[off..].to_vec()) {
                    bail!("duplicate key {i}");
                }
                let v = encode_value(&value, v, dst)
                    .with_context(|| format!("failed to encode value {i}"))?;
                let entry: BTreeMap<_, _> = [k, v]
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, t)| t.map(|t| (i, t)))
                    .collect();
                if !entry.is_empty() {
                    nested.insert(i, Box::new(entry));
                }
            }
            if nested.is_empty() {
                return Ok(None);
            }
            Ok(Some(Box::new(nested)))
        }
        (Val::FixedLengthList(vs), Type::FixedLengthList(ty)) => {
            if u32::try_from(vs.len()) != Ok(ty.len()) {
                bail!("list has {} elements, expected {}", vs.len(), ty.len());
            }
            encode_seq(iter::repeat(ty.ty()), vs, dst)
        }
        (Val::Record(vs), Type::Record(ty)) => {
            if vs.len() != ty.fields().len() {
                bail!(
                    "record has {} fields, expected {}",
                    vs.len(),
                    ty.fields().len()
                );
            }
            for ((name, ..), Field { name: field, .. }) in zip(&vs, ty.fields()) {
                if name != field {
                    bail!("record field `{name}` does not match `{field}`");
                }
            }
            let tys = ty.fields().map(|Field { ty, .. }| ty);
            encode_seq(tys, vs.into_iter().map(|(.., v)| v), dst)
        }
        (Val::Tuple(vs), Type::Tuple(ty)) => {
            if vs.len() != ty.types().len() {
                bail!(
                    "tuple has {} elements, expected {}",
                    vs.len(),
                    ty.types().len()
                );
            }
            encode_seq(ty.types(), vs, dst)
        }
        (Val::Variant(case, v), Type::Variant(ty)) => {
            let (i, ty) = zip(0u32.., ty.cases())
                .find_map(|(i, Case { name, ty })| (name == case).then_some((i, ty)))
                .with_context(|| format!("unknown variant case `{case}`"))?;
            Leb128Encoder.encode(i, dst)?;
            encode_payload(ty, v, dst)
        }
        (Val::Enum(case), Type::Enum(ty)) => {
            let i = zip(0u32.., ty.names())
                .find_map(|(i, name)| (name == case).then_some(i))
                .with_context(|| format!("unknown enum case `{case}`"))?;
            Leb128Encoder.encode(i, dst)?;
            Ok(None)
        }
        (Val::Option(None), Type::Option(..)) => {
            dst.put_u8(0);
            Ok(None)
        }
        (Val::Option(Some(v)), Type::Option(ty)) => {
            dst.put_u8(1);
            encode_value(&ty.ty(), *v, dst).context("failed to encode `option::some` value")
        }
        (Val::Result(Ok(v)), Type::Result(ty)) => {
            dst.put_u8(0);
            encode_payload(ty.ok(), v, dst)
        }
        (Val::Result(Err(v)), Type::Result(ty)) => {
            dst.put_u8(1);
            encode_payload(ty.err(), v, dst)
        }
        (Val::Flags(vs), Type::Flags(ty)) => {
            let names = ty.names();
            let vs: HashSet<&str> = vs.iter().map(String::as_str).collect();
            let off = dst.len();
            dst.put_bytes(0, names.len().div_ceil(8).max(1));
            for (i, name) in names.enumerate() {
                if vs.contains(name) {
                    dst[off + i / 8] |= 1 << (i % 8);
                }
            }
            Ok(None)
        }
        (Val::Resource(v), Type::Own(..) | Type::Borrow(..)) => {
            CoreVecEncoderBytes.encode(v, dst)?;
            Ok(None)
        }
        (Val::Future(v), Type::Future(ty)) => {
            let ty = ty.ty();
            let v = v.map_ok(|v| Payload(ty, v));
            dst.put_u8(0);
            Ok(Some(Box::new(FutureEncoder::pending(v))))
        }
        (Val::Stream(vs), Type::Stream(ty)) => {
            let Some(ty) = ty.ty() else {
                bail!("`stream` without a payload type not supported")
            };
            let vs = vs.map(move |vs| {
                vs.into_iter()
                    .map(|v| Payload(Some(ty.clone()), Some(v)))
                    .collect::<Vec<_>>()
            });
            dst.put_u8(0);
            Ok(Some(Box::new(StreamEncoder::pending(vs))))
        }
        _ => bail!("value type mismatch"),
    }
}

/// Resume point of a [`TupleDecoder`] within a partially-received value
enum Partial {
    Decoder(Box<dyn Any + Send>),
    /// Decoded elements of a sequence of `n` elements
    Seq(usize, Vec<Val>),
    /// Decoded entries of a `map` of `n` entries
    Map(usize, Vec<(Val, Val)>),
    /// `variant`, `option` or `result` case awaiting its payload of type `Type`
    Case(Val, Type),
    /// `future` or `stream`, whose `ready` status was decoded
    Ready,
}

/// Decodes [`Val`]s of types `tys`, e.g. function parameters.
///
/// Pending asynchronous values are received using [`Receive::poll_receive`].
pub struct TupleDecoder {
    tys: Arc<[Type]>,
    max_len: u32,
    /// Resume points, innermost first
    state: Vec<Partial>,
    /// Decoders of the pending asynchronous values keyed by their paths
    nested: BTreeMap<Box<[usize]>, Box<dyn Receive + Send>>,
}

impl TupleDecoder {
    #[must_use]
    pub fn new(tys: Arc<[Type]>) -> Self {
        Self {
            tys,
            max_len: DEFAULT_MAX_LEN,
            state: Vec::default(),
            nested: BTreeMap::default(),
        }
    }

    /// Rejects strings and resource handles longer than `max_len` bytes and lists, maps and
    /// `stream` chunks longer than `max_len` elements, [`DEFAULT_MAX_LEN`] by default.
    #[must_use]
    pub fn with_max_len(mut self, max_len: u32) -> Self {
        self.max_len = max_len;
        self
    }
}

impl Receive for TupleDecoder {
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
    state: &mut Vec<Partial>,
    dec: T,
) -> std::io::Result<Option<T::Item>>
where
    T: Decoder + Send + 'static,
    std::io::Error: From<T::Error>,
{
    let mut dec = match state.pop() {
        Some(Partial::Decoder(prev)) => prev.downcast().map_or(dec, |prev| *prev),
        _ => dec,
    };
    let v = dec.decode(src)?;
    if v.is_none() {
        state.push(Partial::Decoder(Box::new(dec)));
    }
    Ok(v)
}

impl TupleDecoder {
    fn decode_len(&self, src: &mut BytesMut) -> std::io::Result<Option<usize>> {
        let Some(n) = Leb128DecoderU32.decode(src)? else {
            return Ok(None);
        };
        if n > self.max_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("length `{n}` exceeds the maximum of `{}`", self.max_len),
            ));
        }
        Ok(Some(usize::try_from(n).unwrap_or(usize::MAX)))
    }

    /// Decodes a value of type `ty`, whose pending asynchronous values are received on paths
    /// relative to it
    fn decode_val(&mut self, src: &mut BytesMut, ty: &Type) -> std::io::Result<Option<Val>> {
        let mut state = mem::take(&mut self.state);
        let v = self.decode_ty(src, ty, &mut Vec::default(), &mut state)?;
        self.state = state;
        Ok(v)
    }

    fn decode_map(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Vec<Partial>,
        key: &Type,
        value: &Type,
    ) -> std::io::Result<Option<Vec<(Val, Val)>>> {
        let (n, mut vs) = match state.pop() {
            Some(Partial::Map(n, vs)) => (n, vs),
            _ => {
                let Some(n) = self.decode_len(src)? else {
                    return Ok(None);
                };
                let cap = DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<(Val, Val)>();
                (n, Vec::with_capacity(n.min(cap)))
            }
        };
        while vs.len() < n {
            path.push(vs.len());
            let kv = self.decode_seq(src, path, state, Some(2), [key, value])?;
            path.pop();
            let Some(Ok([k, v])) = kv.map(<[Val; 2]>::try_from) else {
                state.push(Partial::Map(n, vs));
                return Ok(None);
            };
            vs.push((k, v));
        }
        Ok(Some(vs))
    }

    fn decode_seq(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Vec<Partial>,
        len: Option<usize>,
        tys: impl IntoIterator<Item: Borrow<Type>>,
    ) -> std::io::Result<Option<Vec<Val>>> {
        let (n, mut vs) = match state.pop() {
            Some(Partial::Seq(n, vs)) => (n, vs),
            _ => {
                let n = if let Some(n) = len {
                    n
                } else {
                    let Some(n) = self.decode_len(src)? else {
                        return Ok(None);
                    };
                    n
                };
                let vs =
                    Vec::with_capacity(n.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<Val>()));
                (n, vs)
            }
        };
        for ty in tys.into_iter().take(n).skip(vs.len()) {
            path.push(vs.len());
            let v = self.decode_ty(src, ty.borrow(), path, state)?;
            path.pop();
            let Some(v) = v else {
                state.push(Partial::Seq(n, vs));
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
        state: &mut Vec<Partial>,
        case: impl FnOnce(&mut BytesMut) -> std::io::Result<Option<(Val, Option<Type>)>>,
    ) -> std::io::Result<Option<Val>> {
        let (mut v, ty) = match state.pop() {
            Some(Partial::Case(v, ty)) => (v, ty),
            _ => {
                let Some((v, ty)) = case(src)? else {
                    return Ok(None);
                };
                let Some(ty) = ty else {
                    return Ok(Some(v));
                };
                (v, ty)
            }
        };
        let Some(p) = self.decode_ty(src, &ty, path, state)? else {
            state.push(Partial::Case(v, ty));
            return Ok(None);
        };
        if let Val::Variant(.., x) | Val::Option(x) | Val::Result(Ok(x) | Err(x)) = &mut v {
            *x = Some(Box::new(p));
        }
        Ok(Some(v))
    }

    fn decode_future(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Vec<Partial>,
        ty: Option<Type>,
    ) -> std::io::Result<Option<Val>> {
        if !matches!(state.pop(), Some(Partial::Ready)) {
            let Some(status) = decode_status(src, "future")? else {
                return Ok(None);
            };
            if status == 0 {
                let (v, r) = pending_future(ty, self.max_len);
                self.nested.insert(path.as_slice().into(), r);
                return Ok(Some(v));
            }
        }
        let v = if let Some(ty) = ty {
            path.push(0);
            let v = self.decode_ty(src, &ty, path, state)?;
            path.pop();
            v.map(Some)
        } else {
            Some(None)
        };
        let Some(v) = v else {
            state.push(Partial::Ready);
            return Ok(None);
        };
        Ok(Some(Val::Future(Box::pin(future::ready(Ok(v))))))
    }

    fn decode_stream(
        &mut self,
        src: &mut BytesMut,
        path: &mut Vec<usize>,
        state: &mut Vec<Partial>,
        ty: Option<Type>,
    ) -> std::io::Result<Option<Val>> {
        let Some(ty) = ty else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`stream` without a payload type not supported",
            ));
        };
        if !matches!(state.pop(), Some(Partial::Ready)) {
            let Some(status) = decode_status(src, "stream")? else {
                return Ok(None);
            };
            if status == 0 {
                let (v, r) = pending_stream(ty, self.max_len);
                self.nested.insert(path.as_slice().into(), r);
                return Ok(Some(v));
            }
        }
        let Some(vs) = self.decode_seq(src, path, state, None, iter::repeat(&ty))? else {
            state.push(Partial::Ready);
            return Ok(None);
        };
        Ok(Some(Val::Stream(Box::pin(stream::iter(
            (!vs.is_empty()).then_some(vs),
        )))))
    }

    #[allow(clippy::too_many_lines)]
    fn decode_ty(
        &mut self,
        src: &mut BytesMut,
        ty: &Type,
        path: &mut Vec<usize>,
        state: &mut Vec<Partial>,
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
            Type::Map(ty) => {
                let vs = self.decode_map(src, path, state, &ty.key(), &ty.value())?;
                Ok(vs.map(Val::Map))
            }
            Type::FixedLengthList(ty) => {
                let n = usize::try_from(ty.len()).unwrap_or(usize::MAX);
                let ty = ty.ty();
                let vs = self.decode_seq(src, path, state, Some(n), iter::repeat(&ty))?;
                Ok(vs.map(Val::FixedLengthList))
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
                let len = names.len();
                let n = len.div_ceil(8).max(1);
                if src.len() < n {
                    src.reserve(n.saturating_sub(src.len()));
                    return Ok(None);
                }
                let buf = src.split_to(n);
                if (len..n * 8).any(|i| buf[i / 8] & (1 << (i % 8)) != 0) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid flags bits",
                    ));
                }
                let vs = names
                    .enumerate()
                    .filter(|(i, ..)| buf[i / 8] & (1 << (i % 8)) != 0)
                    .map(|(.., name)| name.to_string())
                    .collect();
                Ok(Some(Val::Flags(vs)))
            }
            Type::Own(..) | Type::Borrow(..) => {
                let dec = <CoreVecDecoderBytes>::with_max_len(self.max_len);
                let v = decode_with(src, state, dec)?;
                Ok(v.map(Val::Resource))
            }
            Type::Future(ty) => self.decode_future(src, path, state, ty.ty()),
            Type::Stream(ty) => self.decode_stream(src, path, state, ty.ty()),
            Type::ErrorContext => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "`error-context` not supported",
            )),
        }
    }
}

impl Decoder for TupleDecoder {
    type Item = Vec<Val>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        let n = self.state.is_empty().then_some(src.len());
        let mut state = mem::take(&mut self.state);
        let mut path = Vec::default();
        let tys = Arc::clone(&self.tys);
        let vs = self
            .decode_seq(src, &mut path, &mut state, Some(tys.len()), tys.iter())
            .map_err(|err| {
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
        Ok(vs)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<Val>>> {
        let vs = self.decode(src)?;
        if vs.is_none() && (!self.state.is_empty() || !src.is_empty()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "value incomplete",
            ));
        }
        Ok(vs)
    }
}
