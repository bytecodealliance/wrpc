//! Value encoding

use core::fmt::Display;
use core::iter::zip;
use core::task::{Context, Poll};

use std::borrow::Cow;
use std::sync::Arc;

use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder;
use tracing::instrument;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{CoreNameEncoder, Leb128Encoder, Utf8Codec};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue};
use wrpc_transport::Transmit;

/// [Encoder] of values of type `T`
///
/// Resources, futures and streams are not supported.
///
/// ```
/// use bytes::BytesMut;
/// use tokio_util::codec::Encoder as _;
/// use wasm_wave::value::{Type, Value};
/// use wasm_wave::wasm::WasmValue as _;
/// use wrpc_wave::ValueEncoder;
///
/// let mut buf = BytesMut::default();
/// ValueEncoder::new(Type::U32).encode(&Value::make_u32(42), &mut buf).unwrap();
/// assert_eq!(buf.as_ref(), [42]);
/// ```
#[derive(Debug)]
pub struct ValueEncoder<T: WasmType> {
    ty: T,
}

impl<T: WasmType> ValueEncoder<T> {
    /// Creates a new [`ValueEncoder`] of values of type `ty`
    #[must_use]
    pub fn new(ty: T) -> Self {
        Self { ty }
    }
}

impl<V, T> Encoder<&V> for ValueEncoder<T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(&mut self, v: &V, dst: &mut BytesMut) -> std::io::Result<()> {
        encode_value(&self.ty, v, dst)
    }
}

impl<T: WasmType> Transmit for ValueEncoder<T> {
    fn poll_transmit(
        &mut self,
        _cx: &mut Context<'_>,
        _path: &mut Vec<usize>,
        _dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        true
    }
}

/// [Encoder] of tuples of values of types `tys`, e.g. function parameters
#[derive(Debug)]
pub struct TupleEncoder<T: WasmType> {
    tys: Arc<[T]>,
}

impl<T: WasmType> TupleEncoder<T> {
    /// Creates a new [`TupleEncoder`] of tuples of values of types `tys`
    #[must_use]
    pub fn new(tys: Arc<[T]>) -> Self {
        Self { tys }
    }
}

impl<V, T> Encoder<&[V]> for TupleEncoder<T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(&mut self, vs: &[V], dst: &mut BytesMut) -> std::io::Result<()> {
        if vs.len() != self.tys.len() {
            return Err(error_invalid_input(format!(
                "expected {} values, got {}",
                self.tys.len(),
                vs.len()
            )));
        }
        for (ty, v) in zip(self.tys.iter(), vs) {
            encode_value(ty, v, dst)?;
        }
        Ok(())
    }
}

impl<T: WasmType> Transmit for TupleEncoder<T> {
    fn poll_transmit(
        &mut self,
        _cx: &mut Context<'_>,
        _path: &mut Vec<usize>,
        _dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        true
    }
}

fn error_invalid_input(msg: impl Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string())
}

fn encode_discriminant(i: usize, dst: &mut BytesMut) -> std::io::Result<()> {
    let i = u32::try_from(i).map_err(error_invalid_input)?;
    Leb128Encoder.encode(i, dst)
}

fn encode_payload<T: WasmValue>(
    ty: Option<T::Type>,
    v: Option<Cow<'_, T>>,
    dst: &mut BytesMut,
) -> std::io::Result<()> {
    match (ty, v) {
        (Some(ty), Some(v)) => encode_value(&ty, &*v, dst),
        (None, None) => Ok(()),
        (Some(..), None) => Err(error_invalid_input("payload missing")),
        (None, Some(..)) => Err(error_invalid_input("unexpected payload")),
    }
}

pub(crate) fn encode_value<T: WasmValue>(
    ty: &T::Type,
    v: &T,
    dst: &mut BytesMut,
) -> std::io::Result<()> {
    let kind = ty.kind();
    if v.kind() != kind {
        return Err(error_invalid_input(format!(
            "expected `{kind}` value, got `{}`",
            v.kind()
        )));
    }
    match kind {
        WasmTypeKind::Bool => BoolCodec.encode(v.unwrap_bool(), dst),
        WasmTypeKind::S8 => S8Codec.encode(v.unwrap_s8(), dst),
        WasmTypeKind::U8 => U8Codec.encode(v.unwrap_u8(), dst),
        WasmTypeKind::S16 => Leb128Encoder.encode(v.unwrap_s16(), dst),
        WasmTypeKind::U16 => Leb128Encoder.encode(v.unwrap_u16(), dst),
        WasmTypeKind::S32 => Leb128Encoder.encode(v.unwrap_s32(), dst),
        WasmTypeKind::U32 => Leb128Encoder.encode(v.unwrap_u32(), dst),
        WasmTypeKind::S64 => Leb128Encoder.encode(v.unwrap_s64(), dst),
        WasmTypeKind::U64 => Leb128Encoder.encode(v.unwrap_u64(), dst),
        WasmTypeKind::F32 => F32Codec.encode(v.unwrap_f32(), dst),
        WasmTypeKind::F64 => F64Codec.encode(v.unwrap_f64(), dst),
        WasmTypeKind::Char => Utf8Codec.encode(v.unwrap_char(), dst),
        WasmTypeKind::String => CoreNameEncoder.encode(v.unwrap_string(), dst),
        WasmTypeKind::List => {
            let ty = ty
                .list_element_type()
                .ok_or_else(|| error_invalid_input("list type missing element type"))?;
            let vs: Vec<_> = v.unwrap_list().collect();
            encode_discriminant(vs.len(), dst)?;
            for v in vs {
                encode_value(&ty, &*v, dst)?;
            }
            Ok(())
        }
        WasmTypeKind::Record => {
            let vs: Vec<_> = v.unwrap_record().collect();
            let tys: Vec<_> = ty.record_fields().collect();
            if vs.len() != tys.len() {
                return Err(error_invalid_input(format!(
                    "record has {} fields, expected {}",
                    vs.len(),
                    tys.len()
                )));
            }
            for ((.., v), (.., ty)) in zip(vs, tys) {
                encode_value(&ty, &*v, dst)?;
            }
            Ok(())
        }
        WasmTypeKind::Tuple => {
            let vs: Vec<_> = v.unwrap_tuple().collect();
            let tys: Vec<_> = ty.tuple_element_types().collect();
            if vs.len() != tys.len() {
                return Err(error_invalid_input(format!(
                    "tuple has {} elements, expected {}",
                    vs.len(),
                    tys.len()
                )));
            }
            for (v, ty) in zip(vs, tys) {
                encode_value(&ty, &*v, dst)?;
            }
            Ok(())
        }
        WasmTypeKind::Variant => {
            let (case, v) = v.unwrap_variant();
            let (i, (.., ty)) = ty
                .variant_cases()
                .enumerate()
                .find(|(.., (name, ..))| *name == case)
                .ok_or_else(|| error_invalid_input(format!("unknown variant case `{case}`")))?;
            encode_discriminant(i, dst)?;
            encode_payload(ty, v, dst)
        }
        WasmTypeKind::Enum => {
            let case = v.unwrap_enum();
            let i = ty
                .enum_cases()
                .position(|name| name == case)
                .ok_or_else(|| error_invalid_input(format!("unknown enum case `{case}`")))?;
            encode_discriminant(i, dst)
        }
        WasmTypeKind::Option => {
            let v = v.unwrap_option();
            dst.reserve(1);
            dst.put_u8(v.is_some().into());
            if v.is_none() {
                return Ok(());
            }
            encode_payload(ty.option_some_type(), v, dst)
        }
        WasmTypeKind::Result => {
            let (ok, err) = ty
                .result_types()
                .ok_or_else(|| error_invalid_input("result type missing ok/err types"))?;
            dst.reserve(1);
            match v.unwrap_result() {
                Ok(v) => {
                    dst.put_u8(0);
                    encode_payload(ok, v, dst)
                }
                Err(v) => {
                    dst.put_u8(1);
                    encode_payload(err, v, dst)
                }
            }
        }
        WasmTypeKind::Flags => {
            let names: Vec<_> = ty.flags_names().collect();
            let start = dst.len();
            dst.resize(start.saturating_add(names.len().div_ceil(8)), 0);
            for flag in v.unwrap_flags() {
                let i = names
                    .iter()
                    .position(|name| *name == flag)
                    .ok_or_else(|| error_invalid_input(format!("unknown flag `{flag}`")))?;
                dst[start.saturating_add(i / 8)] |= 1 << (i % 8);
            }
            Ok(())
        }
        kind => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("unsupported value type `{kind}`"),
        )),
    }
}
