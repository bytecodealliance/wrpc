//! Value decoding

use core::borrow::Borrow;
use core::fmt::Display;
use core::iter::{self, zip};
use core::mem;
use core::task::{Context, Poll};

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use bytes::{Buf as _, BytesMut};
use tokio_util::codec::Decoder;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderI16, Leb128DecoderI32,
    Leb128DecoderI64, Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Utf8Codec,
};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue};
use wrpc_core::{Receive, error_unknown_path};

/// Default `max_len` of [`ValueDecoder`] and [`TupleDecoder`]
pub const DEFAULT_MAX_LEN: u32 = 1 << 24;

/// Progress of decoding a partially received value
enum Partial<V> {
    String(CoreNameDecoder),
    Seq {
        n: usize,
        vs: Vec<V>,
        elem: Option<Box<Self>>,
    },
    Case {
        discriminant: usize,
        payload: Option<Box<Self>>,
    },
}

/// [Decoder] of values of type `ty`
///
/// Resources, futures and streams are not supported.
pub struct ValueDecoder<V: WasmValue> {
    ty: V::Type,
    max_len: u32,
    state: Option<Box<Partial<V>>>,
}

impl<V: WasmValue> ValueDecoder<V> {
    /// Creates a new [`ValueDecoder`] of values of type `ty`
    #[must_use]
    pub fn new(ty: V::Type) -> Self {
        Self {
            ty,
            max_len: DEFAULT_MAX_LEN,
            state: None,
        }
    }

    /// Rejects strings longer than `max_len` bytes and lists and maps longer than `max_len`
    /// elements, [`DEFAULT_MAX_LEN`] by default.
    #[must_use]
    pub fn with_max_len(mut self, max_len: u32) -> Self {
        self.max_len = max_len;
        self
    }
}

impl<V: WasmValue> Decoder for ValueDecoder<V> {
    type Item = V;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<V>> {
        let n = self.state.is_none().then_some(src.len());
        let v = decode_ty(&self.ty, src, &mut self.state, self.max_len)?;
        if n == Some(src.len()) {
            self.state = None;
        }
        Ok(v)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<V>> {
        let v = self.decode(src)?;
        decode_eof(v, self.state.is_some(), src)
    }
}

impl<V: WasmValue> Receive for ValueDecoder<V> {
    fn poll_receive(
        &mut self,
        _cx: &mut Context<'_>,
        path: &[usize],
        _src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(error_unknown_path(path)))
    }

    fn is_complete(&self) -> bool {
        true
    }
}

/// [Decoder] of tuples of values of types `tys`, e.g. function results
pub struct TupleDecoder<V: WasmValue> {
    tys: Arc<[V::Type]>,
    max_len: u32,
    state: Option<Box<Partial<V>>>,
}

impl<V: WasmValue> TupleDecoder<V> {
    /// Creates a new [`TupleDecoder`] of tuples of values of types `tys`
    #[must_use]
    pub fn new(tys: Arc<[V::Type]>) -> Self {
        Self {
            tys,
            max_len: DEFAULT_MAX_LEN,
            state: None,
        }
    }

    /// Rejects strings longer than `max_len` bytes and lists and maps longer than `max_len`
    /// elements, [`DEFAULT_MAX_LEN`] by default.
    #[must_use]
    pub fn with_max_len(mut self, max_len: u32) -> Self {
        self.max_len = max_len;
        self
    }
}

impl<V: WasmValue> Decoder for TupleDecoder<V> {
    type Item = Vec<V>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<V>>> {
        let n = self.state.is_none().then_some(src.len());
        let vs = decode_seq(
            src,
            &mut self.state,
            self.max_len,
            |_| Ok(Some(self.tys.len())),
            self.tys.iter(),
        )?;
        if n == Some(src.len()) {
            self.state = None;
        }
        Ok(vs)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<V>>> {
        let vs = self.decode(src)?;
        decode_eof(vs, self.state.is_some(), src)
    }
}

impl<V: WasmValue> Receive for TupleDecoder<V> {
    fn poll_receive(
        &mut self,
        _cx: &mut Context<'_>,
        path: &[usize],
        _src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(error_unknown_path(path)))
    }

    fn is_complete(&self) -> bool {
        true
    }
}

fn error_invalid_data(msg: impl Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

fn decode_eof<T>(v: Option<T>, partial: bool, src: &BytesMut) -> std::io::Result<Option<T>> {
    if v.is_none() && (partial || !src.is_empty()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "value incomplete",
        ));
    }
    Ok(v)
}

fn decode_leaf<D: Decoder<Error = std::io::Error>, V>(
    mut dec: D,
    src: &mut BytesMut,
    f: impl FnOnce(D::Item) -> V,
) -> std::io::Result<Option<V>> {
    let v = dec.decode(src)?;
    Ok(v.map(f))
}

fn decode_discriminant(src: &mut BytesMut) -> std::io::Result<Option<usize>> {
    let Some(v) = Leb128DecoderU32.decode(src)? else {
        return Ok(None);
    };
    Ok(Some(usize::try_from(v).unwrap_or(usize::MAX)))
}

fn decode_status(src: &mut BytesMut) -> std::io::Result<Option<usize>> {
    let Some(&status) = src.first() else {
        src.reserve(1);
        return Ok(None);
    };
    if status > 1 {
        return Err(error_invalid_data(format!(
            "invalid status byte `{status}`"
        )));
    }
    src.advance(1);
    Ok(Some(status.into()))
}

fn decode_len(src: &mut BytesMut, max_len: u32) -> std::io::Result<Option<usize>> {
    let Some(n) = Leb128DecoderU32.decode(src)? else {
        return Ok(None);
    };
    if n > max_len {
        return Err(error_invalid_data(format!(
            "length `{n}` exceeds the maximum of `{max_len}`"
        )));
    }
    Ok(Some(usize::try_from(n).unwrap_or(usize::MAX)))
}

fn decode_seq<T: WasmValue>(
    src: &mut BytesMut,
    state: &mut Option<Box<Partial<T>>>,
    max_len: u32,
    len: impl FnOnce(&mut BytesMut) -> std::io::Result<Option<usize>>,
    tys: impl IntoIterator<Item: Borrow<T::Type>>,
) -> std::io::Result<Option<Vec<T>>> {
    let (n, mut vs, mut elem) = match state.take().map(|state| *state) {
        Some(Partial::Seq { n, vs, elem }) => (n, vs, elem),
        _ => {
            let Some(n) = len(src)? else {
                return Ok(None);
            };
            let cap = n
                .min(src.len())
                .min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<T>().max(1));
            (n, Vec::with_capacity(cap), None)
        }
    };
    for ty in tys.into_iter().take(n).skip(vs.len()) {
        let Some(v) = decode_ty(ty.borrow(), src, &mut elem, max_len)? else {
            *state = Some(Box::new(Partial::Seq { n, vs, elem }));
            return Ok(None);
        };
        vs.push(v);
    }
    Ok(Some(vs))
}

fn decode_case<T: WasmValue>(
    src: &mut BytesMut,
    state: &mut Option<Box<Partial<T>>>,
    max_len: u32,
    discriminant: impl FnOnce(&mut BytesMut) -> std::io::Result<Option<usize>>,
    payload: impl FnOnce(usize) -> std::io::Result<Option<T::Type>>,
) -> std::io::Result<Option<(usize, Option<T>)>> {
    let (i, mut state_payload) = match state.take().map(|state| *state) {
        Some(Partial::Case {
            discriminant,
            payload,
        }) => (discriminant, payload),
        _ => {
            let Some(i) = discriminant(src)? else {
                return Ok(None);
            };
            (i, None)
        }
    };
    let Some(ty) = payload(i)? else {
        return Ok(Some((i, None)));
    };
    let Some(v) = decode_ty(&ty, src, &mut state_payload, max_len)? else {
        *state = Some(Box::new(Partial::Case {
            discriminant: i,
            payload: state_payload,
        }));
        return Ok(None);
    };
    Ok(Some((i, Some(v))))
}

fn decode_ty<T: WasmValue>(
    ty: &T::Type,
    src: &mut BytesMut,
    state: &mut Option<Box<Partial<T>>>,
    max_len: u32,
) -> std::io::Result<Option<T>> {
    match ty.kind() {
        WasmTypeKind::Bool => decode_leaf(BoolCodec, src, T::make_bool),
        WasmTypeKind::S8 => decode_leaf(S8Codec, src, T::make_s8),
        WasmTypeKind::U8 => decode_leaf(U8Codec, src, T::make_u8),
        WasmTypeKind::S16 => decode_leaf(Leb128DecoderI16, src, T::make_s16),
        WasmTypeKind::U16 => decode_leaf(Leb128DecoderU16, src, T::make_u16),
        WasmTypeKind::S32 => decode_leaf(Leb128DecoderI32, src, T::make_s32),
        WasmTypeKind::U32 => decode_leaf(Leb128DecoderU32, src, T::make_u32),
        WasmTypeKind::S64 => decode_leaf(Leb128DecoderI64, src, T::make_s64),
        WasmTypeKind::U64 => decode_leaf(Leb128DecoderU64, src, T::make_u64),
        WasmTypeKind::F32 => decode_leaf(F32Codec, src, T::make_f32),
        WasmTypeKind::F64 => decode_leaf(F64Codec, src, T::make_f64),
        WasmTypeKind::Char => decode_leaf(Utf8Codec, src, T::make_char),
        WasmTypeKind::String => {
            let mut dec = match state.take().map(|state| *state) {
                Some(Partial::String(dec)) => dec,
                _ => CoreNameDecoder::with_max_len(max_len),
            };
            let Some(s) = dec.decode(src)? else {
                *state = Some(Box::new(Partial::String(dec)));
                return Ok(None);
            };
            Ok(Some(T::make_string(Cow::Owned(s))))
        }
        WasmTypeKind::List => {
            let elem = ty
                .list_element_type()
                .ok_or_else(|| error_invalid_data("list type missing element type"))?;
            let len = |src: &mut BytesMut| decode_len(src, max_len);
            let Some(vs) = decode_seq(src, state, max_len, len, iter::repeat(&elem))? else {
                return Ok(None);
            };
            T::make_list(ty, vs).map(Some).map_err(error_invalid_data)
        }
        WasmTypeKind::Map => {
            let (k, v) = ty
                .map_key_value_types()
                .ok_or_else(|| error_invalid_data("map type missing key/value types"))?;
            let len = |src: &mut BytesMut| {
                let n = decode_len(src, max_len)?;
                Ok(n.map(|n| n.saturating_mul(2)))
            };
            let tys = iter::repeat([&k, &v]).flatten();
            let Some(vs) = decode_seq(src, state, max_len, len, tys)? else {
                return Ok(None);
            };
            let mut vs = vs.into_iter();
            let entries: Vec<_> = iter::from_fn(|| vs.next().zip(vs.next())).collect();
            let mut keys = HashSet::with_capacity(entries.len());
            for (k, ..) in &entries {
                let k = wasm_wave::to_string(k).map_err(error_invalid_data)?;
                if !keys.insert(k) {
                    return Err(error_invalid_data("map contains duplicate keys"));
                }
            }
            T::make_map(ty, entries)
                .map(Some)
                .map_err(error_invalid_data)
        }
        WasmTypeKind::Record => {
            let (names, tys): (Vec<_>, Vec<_>) = ty.record_fields().unzip();
            let Some(vs) = decode_seq(src, state, max_len, |_| Ok(Some(tys.len())), &tys)? else {
                return Ok(None);
            };
            let fields = zip(names.iter().map(AsRef::as_ref), vs);
            T::make_record(ty, fields)
                .map(Some)
                .map_err(error_invalid_data)
        }
        WasmTypeKind::Tuple => {
            let tys: Vec<_> = ty.tuple_element_types().collect();
            let Some(vs) = decode_seq(src, state, max_len, |_| Ok(Some(tys.len())), &tys)? else {
                return Ok(None);
            };
            T::make_tuple(ty, vs).map(Some).map_err(error_invalid_data)
        }
        WasmTypeKind::Variant => {
            let case = |i| {
                ty.variant_cases().nth(i).ok_or_else(|| {
                    error_invalid_data(format!("unknown variant discriminant `{i}`"))
                })
            };
            let Some((i, v)) = decode_case(src, state, max_len, decode_discriminant, |i| {
                let (.., ty) = case(i)?;
                Ok(ty)
            })?
            else {
                return Ok(None);
            };
            let (name, ..) = case(i)?;
            T::make_variant(ty, &name, v)
                .map(Some)
                .map_err(error_invalid_data)
        }
        WasmTypeKind::Enum => {
            let Some(i) = decode_discriminant(src)? else {
                return Ok(None);
            };
            let name = ty
                .enum_cases()
                .nth(i)
                .ok_or_else(|| error_invalid_data(format!("unknown enum discriminant `{i}`")))?;
            T::make_enum(ty, &name)
                .map(Some)
                .map_err(error_invalid_data)
        }
        WasmTypeKind::Option => {
            let Some((.., v)) = decode_case(src, state, max_len, decode_status, |i| {
                if i == 0 {
                    return Ok(None);
                }
                let ty = ty
                    .option_some_type()
                    .ok_or_else(|| error_invalid_data("option type missing some type"))?;
                Ok(Some(ty))
            })?
            else {
                return Ok(None);
            };
            T::make_option(ty, v).map(Some).map_err(error_invalid_data)
        }
        WasmTypeKind::Result => {
            let (ok, err) = ty
                .result_types()
                .ok_or_else(|| error_invalid_data("result type missing ok/err types"))?;
            let Some((i, v)) = decode_case(src, state, max_len, decode_status, |i| {
                Ok(if i == 0 { ok } else { err })
            })?
            else {
                return Ok(None);
            };
            let v = if i == 0 { Ok(v) } else { Err(v) };
            T::make_result(ty, v).map(Some).map_err(error_invalid_data)
        }
        WasmTypeKind::Flags => {
            let names: Vec<_> = ty.flags_names().collect();
            let n = names.len().div_ceil(8);
            if src.len() < n {
                src.reserve(n.saturating_sub(src.len()));
                return Ok(None);
            }
            let buf = src.split_to(n);
            if (names.len()..n * 8).any(|i| buf[i / 8] & (1 << (i % 8)) != 0) {
                return Err(error_invalid_data("invalid flags bits"));
            }
            let flags = names
                .iter()
                .enumerate()
                .filter(|(i, ..)| buf[i / 8] & (1 << (i % 8)) != 0)
                .map(|(.., name)| name.as_ref());
            T::make_flags(ty, flags)
                .map(Some)
                .map_err(error_invalid_data)
        }
        kind => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("unsupported value type `{kind}`"),
        )),
    }
}
