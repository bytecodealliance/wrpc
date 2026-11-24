//! Value encoding

use core::error::Error;
use core::fmt;
use core::iter::zip;
use core::task::{Context, Poll};

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder;
use tracing::instrument;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{CoreNameEncoder, Leb128Encoder, Utf8Codec};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue};
use wasm_wave::writer::WriterError;
use wrpc_core::Transmit;

/// Error of encoding a value
#[derive(Debug)]
pub enum EncodeError {
    /// A primitive value, string, length or discriminant could not be encoded
    Primitive(std::io::Error),
    /// The kind of the value does not match the type
    TypeMismatch {
        /// Kind of the type
        expected: WasmTypeKind,
        /// Kind of the value
        got: WasmTypeKind,
    },
    /// The number of tuple elements, record fields or function values does not match the type
    LengthMismatch {
        /// Number of elements of the type
        expected: usize,
        /// Number of elements of the value
        got: usize,
    },
    /// A record field name does not match the field of the type at its position
    FieldMismatch {
        /// Field name of the type
        expected: String,
        /// Field name of the value
        got: String,
    },
    /// A variant or enum case is not a case of the type
    UnknownCase(String),
    /// A flag is not a flag of the type
    UnknownFlag(String),
    /// A case payload is missing or present, contrary to the type
    PayloadMismatch,
    /// A list or map length or a discriminant does not fit in a `u32`
    Overflow,
    /// The type is missing a nested type
    InvalidType,
    /// Values of the type are not supported
    UnsupportedType(WasmTypeKind),
    /// A map contains duplicate keys
    DuplicateKey,
    /// A map key could not be formatted to check for duplicates
    Key(WriterError),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primitive(..) => f.write_str("failed to encode primitive value"),
            Self::TypeMismatch { expected, got } => {
                write!(f, "expected `{expected}` value, got `{got}`")
            }
            Self::LengthMismatch { expected, got } => {
                write!(f, "expected `{expected}` elements, got `{got}`")
            }
            Self::FieldMismatch { expected, got } => {
                write!(f, "expected record field `{expected}`, got `{got}`")
            }
            Self::UnknownCase(case) => write!(f, "unknown case `{case}`"),
            Self::UnknownFlag(flag) => write!(f, "unknown flag `{flag}`"),
            Self::PayloadMismatch => f.write_str("case payload does not match the type"),
            Self::Overflow => f.write_str("length or discriminant does not fit in `u32`"),
            Self::InvalidType => f.write_str("type is missing a nested type"),
            Self::UnsupportedType(kind) => write!(f, "unsupported value type `{kind}`"),
            Self::DuplicateKey => f.write_str("map contains duplicate keys"),
            Self::Key(..) => f.write_str("failed to format map key"),
        }
    }
}

impl Error for EncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Primitive(err) => Some(err),
            Self::Key(err) => Some(err),
            Self::TypeMismatch { .. }
            | Self::LengthMismatch { .. }
            | Self::FieldMismatch { .. }
            | Self::UnknownCase(..)
            | Self::UnknownFlag(..)
            | Self::PayloadMismatch
            | Self::Overflow
            | Self::InvalidType
            | Self::UnsupportedType(..)
            | Self::DuplicateKey => None,
        }
    }
}

impl From<EncodeError> for std::io::Error {
    fn from(err: EncodeError) -> Self {
        let kind = match &err {
            EncodeError::Primitive(err) => err.kind(),
            EncodeError::UnsupportedType(..) => std::io::ErrorKind::Unsupported,
            _ => std::io::ErrorKind::InvalidInput,
        };
        Self::new(kind, err)
    }
}

/// [Encoder] of values of type `T`
///
/// Resources, futures and streams are not supported.
///
/// Its [`std::io::Error`]s wrap a [`EncodeError`], recoverable with `std::io::Error::downcast`.
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
        encode_value(&self.ty, v, dst)?;
        Ok(())
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

    fn is_complete(&self) -> bool {
        true
    }
}

/// [Encoder] of tuples of values of types `tys`, e.g. function parameters
///
/// Its [`std::io::Error`]s wrap a [`EncodeError`], recoverable with `std::io::Error::downcast`.
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
            return Err(EncodeError::LengthMismatch {
                expected: self.tys.len(),
                got: vs.len(),
            }
            .into());
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

    fn is_complete(&self) -> bool {
        true
    }
}

fn encode_leaf<T>(
    mut enc: impl Encoder<T, Error = std::io::Error>,
    v: T,
    dst: &mut BytesMut,
) -> Result<(), EncodeError> {
    enc.encode(v, dst).map_err(EncodeError::Primitive)
}

fn encode_discriminant(i: usize, dst: &mut BytesMut) -> Result<(), EncodeError> {
    let i = u32::try_from(i).map_err(|_| EncodeError::Overflow)?;
    encode_leaf(Leb128Encoder, i, dst)
}

fn encode_payload<T: WasmValue>(
    ty: Option<T::Type>,
    v: Option<Cow<'_, T>>,
    dst: &mut BytesMut,
) -> Result<(), EncodeError> {
    match (ty, v) {
        (Some(ty), Some(v)) => encode_value(&ty, &*v, dst),
        (None, None) => Ok(()),
        (Some(..), None) | (None, Some(..)) => Err(EncodeError::PayloadMismatch),
    }
}

pub(crate) fn encode_value<T: WasmValue>(
    ty: &T::Type,
    v: &T,
    dst: &mut BytesMut,
) -> Result<(), EncodeError> {
    let kind = ty.kind();
    if v.kind() != kind {
        return Err(EncodeError::TypeMismatch {
            expected: kind,
            got: v.kind(),
        });
    }
    match kind {
        WasmTypeKind::Bool => encode_leaf(BoolCodec, v.unwrap_bool(), dst),
        WasmTypeKind::S8 => encode_leaf(S8Codec, v.unwrap_s8(), dst),
        WasmTypeKind::U8 => encode_leaf(U8Codec, v.unwrap_u8(), dst),
        WasmTypeKind::S16 => encode_leaf(Leb128Encoder, v.unwrap_s16(), dst),
        WasmTypeKind::U16 => encode_leaf(Leb128Encoder, v.unwrap_u16(), dst),
        WasmTypeKind::S32 => encode_leaf(Leb128Encoder, v.unwrap_s32(), dst),
        WasmTypeKind::U32 => encode_leaf(Leb128Encoder, v.unwrap_u32(), dst),
        WasmTypeKind::S64 => encode_leaf(Leb128Encoder, v.unwrap_s64(), dst),
        WasmTypeKind::U64 => encode_leaf(Leb128Encoder, v.unwrap_u64(), dst),
        WasmTypeKind::F32 => encode_leaf(F32Codec, v.unwrap_f32(), dst),
        WasmTypeKind::F64 => encode_leaf(F64Codec, v.unwrap_f64(), dst),
        WasmTypeKind::Char => encode_leaf(Utf8Codec, v.unwrap_char(), dst),
        WasmTypeKind::String => encode_leaf(CoreNameEncoder, v.unwrap_string(), dst),
        WasmTypeKind::List => {
            let ty = ty.list_element_type().ok_or(EncodeError::InvalidType)?;
            let vs: Vec<_> = v.unwrap_list().collect();
            encode_discriminant(vs.len(), dst)?;
            for v in vs {
                encode_value(&ty, &*v, dst)?;
            }
            Ok(())
        }
        WasmTypeKind::Map => {
            let (kty, vty) = ty.map_key_value_types().ok_or(EncodeError::InvalidType)?;
            let vs: Vec<_> = v.unwrap_map().collect();
            let mut keys = HashSet::with_capacity(vs.len());
            for (k, ..) in &vs {
                let k = wasm_wave::to_string(&**k).map_err(EncodeError::Key)?;
                if !keys.insert(k) {
                    return Err(EncodeError::DuplicateKey);
                }
            }
            encode_discriminant(vs.len(), dst)?;
            for (k, v) in vs {
                encode_value(&kty, &*k, dst)?;
                encode_value(&vty, &*v, dst)?;
            }
            Ok(())
        }
        WasmTypeKind::Record => {
            let vs: Vec<_> = v.unwrap_record().collect();
            let tys: Vec<_> = ty.record_fields().collect();
            if vs.len() != tys.len() {
                return Err(EncodeError::LengthMismatch {
                    expected: tys.len(),
                    got: vs.len(),
                });
            }
            for ((name, ..), (field, ..)) in zip(&vs, &tys) {
                if name != field {
                    return Err(EncodeError::FieldMismatch {
                        expected: field.to_string(),
                        got: name.to_string(),
                    });
                }
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
                return Err(EncodeError::LengthMismatch {
                    expected: tys.len(),
                    got: vs.len(),
                });
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
                .ok_or_else(|| EncodeError::UnknownCase(case.to_string()))?;
            encode_discriminant(i, dst)?;
            encode_payload(ty, v, dst)
        }
        WasmTypeKind::Enum => {
            let case = v.unwrap_enum();
            let i = ty
                .enum_cases()
                .position(|name| name == case)
                .ok_or_else(|| EncodeError::UnknownCase(case.to_string()))?;
            encode_discriminant(i, dst)
        }
        WasmTypeKind::Option => {
            let v = v.unwrap_option();
            dst.put_u8(v.is_some().into());
            if v.is_none() {
                return Ok(());
            }
            let some = ty.option_some_type().ok_or(EncodeError::InvalidType)?;
            encode_payload(Some(some), v, dst)
        }
        WasmTypeKind::Result => {
            let (ok, err) = ty.result_types().ok_or(EncodeError::InvalidType)?;
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
                    .ok_or_else(|| EncodeError::UnknownFlag(flag.to_string()))?;
                dst[start.saturating_add(i / 8)] |= 1 << (i % 8);
            }
            Ok(())
        }
        kind => Err(EncodeError::UnsupportedType(kind)),
    }
}
