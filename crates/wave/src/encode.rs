//! Value encoding

use core::fmt::Display;
use core::iter::zip;
use core::task::{Context, Poll};

use std::borrow::Cow;

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
/// ValueEncoder::new(&Type::U32).encode(&Value::make_u32(42), &mut buf).unwrap();
/// assert_eq!(buf.as_ref(), [42]);
/// ```
#[derive(Debug)]
pub struct ValueEncoder<'a, T: WasmType> {
    ty: &'a T,
}

impl<'a, T: WasmType> ValueEncoder<'a, T> {
    /// Creates a new [`ValueEncoder`] of values of type `ty`
    #[must_use]
    pub fn new(ty: &'a T) -> Self {
        Self { ty }
    }
}

impl<V, T> Encoder<&V> for ValueEncoder<'_, T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(&mut self, v: &V, dst: &mut BytesMut) -> std::io::Result<()> {
        encode_value(self.ty, v, dst)
    }
}

impl<T: WasmType> Transmit for ValueEncoder<'_, T> {
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
pub struct TupleEncoder<'a, T: WasmType> {
    tys: &'a [T],
}

impl<'a, T: WasmType> TupleEncoder<'a, T> {
    /// Creates a new [`TupleEncoder`] of tuples of values of types `tys`
    #[must_use]
    pub fn new(tys: &'a [T]) -> Self {
        Self { tys }
    }
}

impl<V, T> Encoder<&[V]> for TupleEncoder<'_, T>
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
        for (ty, v) in zip(self.tys, vs) {
            encode_value(ty, v, dst)?;
        }
        Ok(())
    }
}

impl<T: WasmType> Transmit for TupleEncoder<'_, T> {
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

fn encode_value<T: WasmValue>(ty: &T::Type, v: &T, dst: &mut BytesMut) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use wasm_wave::value::{Type, Value};

    #[test]
    fn test_encode_bool_true() -> anyhow::Result<()> {
        let value = Value::make_bool(true);
        let ty = Type::BOOL;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[1u8]);
        Ok(())
    }

    #[test]
    fn test_encode_bool_false() -> anyhow::Result<()> {
        let value = Value::make_bool(false);
        let ty = Type::BOOL;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[0u8]);
        Ok(())
    }

    // ====== Integer Types Tests ======

    #[test]
    fn test_encode_s8_positive() -> anyhow::Result<()> {
        let value = Value::make_s8(42);
        let ty = Type::S8;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[42u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s8_negative() -> anyhow::Result<()> {
        let value = Value::make_s8(-42);
        let ty = Type::S8;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[(-42i8) as u8]);
        Ok(())
    }

    #[test]
    fn test_encode_u8() -> anyhow::Result<()> {
        let value = Value::make_u8(255);
        let ty = Type::U8;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[255u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s16() -> anyhow::Result<()> {
        let value = Value::make_s16(1000);
        let ty = Type::S16;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // 1000 in LEB128: 0xE8 0x07
        assert_eq!(buf.as_ref(), &[0xE8, 0x07]);
        Ok(())
    }

    #[test]
    fn test_encode_u16() -> anyhow::Result<()> {
        let value = Value::make_u16(1000);
        let ty = Type::U16;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // 1000 in LEB128: 0xE8 0x07
        assert_eq!(buf.as_ref(), &[0xE8, 0x07]);
        Ok(())
    }

    #[test]
    fn test_encode_s32() -> anyhow::Result<()> {
        let value = Value::make_s32(100000);
        let ty = Type::S32;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // 100000 in LEB128: 0xA0 0x8D 0x06
        assert_eq!(buf.as_ref(), &[0xA0, 0x8D, 0x06]);
        Ok(())
    }

    #[test]
    fn test_encode_u32() -> anyhow::Result<()> {
        let value = Value::make_u32(42);
        let ty = Type::U32;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // 42 in LEB128 is just [42]
        assert_eq!(buf.as_ref(), &[42u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s64() -> anyhow::Result<()> {
        let value = Value::make_s64(-1);
        let ty = Type::S64;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // -1 in LEB128: 0x7F
        assert_eq!(buf.as_ref(), &[0x7F]);
        Ok(())
    }

    #[test]
    fn test_encode_u64() -> anyhow::Result<()> {
        let value = Value::make_u64(12345);
        let ty = Type::U64;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // 12345 in LEB128: 0xB9 0x60
        assert_eq!(buf.as_ref(), &[0xB9, 0x60]);
        Ok(())
    }

    // ====== Float Types Tests ======

    #[test]
    fn test_encode_f32() -> anyhow::Result<()> {
        let value = Value::make_f32(1.5);
        let ty = Type::F32;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &1.5f32.to_le_bytes());
        Ok(())
    }

    #[test]
    fn test_encode_f64() -> anyhow::Result<()> {
        let value = Value::make_f64(2.5);
        let ty = Type::F64;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &2.5f64.to_le_bytes());
        Ok(())
    }

    // ====== Char Tests ======

    #[test]
    fn test_encode_char() -> anyhow::Result<()> {
        let value = Value::make_char('A');
        let ty = Type::CHAR;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // UTF-8 encoding of 'A' (0x41)
        assert_eq!(buf.as_ref(), &[0x41]);
        Ok(())
    }

    #[test]
    fn test_encode_char_unicode() -> anyhow::Result<()> {
        let value = Value::make_char('😀');
        let ty = Type::CHAR;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // UTF-8 encoding of '😀' (U+1F600): 0xF0 0x9F 0x98 0x80
        assert_eq!(buf.as_ref(), &[0xF0, 0x9F, 0x98, 0x80]);
        Ok(())
    }

    #[test]
    fn test_encode_string() -> anyhow::Result<()> {
        use std::borrow::Cow;
        let value = Value::make_string(Cow::Borrowed("hello"));
        let ty = Type::STRING;
        let mut encoder = ValueEncoder::new(&ty);
        let mut buf = BytesMut::default();
        encoder.encode(&value, &mut buf)?;

        // String encoding: length (5 in LEB128) + "hello"
        assert_eq!(buf.as_ref(), &[5, b'h', b'e', b'l', b'l', b'o']);
        Ok(())
    }

    // ====== List Tests ======

    #[test]
    fn test_encode_list_empty() -> anyhow::Result<()> {
        let element_type = Type::U32;
        let list_type = Type::list(element_type.clone());
        let values: Vec<Value> = vec![];
        let list_value = Value::make_list(&list_type, values)?;

        let mut encoder = ValueEncoder::new(&list_type);
        let mut buf = BytesMut::default();
        encoder.encode(&list_value, &mut buf)?;

        // Empty list: length 0
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    #[test]
    fn test_encode_list() -> anyhow::Result<()> {
        let element_type = Type::U32;
        let list_type = Type::list(element_type.clone());
        let values = vec![Value::make_u32(1), Value::make_u32(2), Value::make_u32(3)];
        let list_value = Value::make_list(&list_type, values)?;

        let mut encoder = ValueEncoder::new(&list_type);
        let mut buf = BytesMut::default();
        encoder.encode(&list_value, &mut buf)?;

        // List encoding: length (3) + elements (1, 2, 3 in LEB128)
        assert_eq!(buf.as_ref(), &[3, 1, 2, 3]);
        Ok(())
    }

    // ====== Record Tests ======

    #[test]
    fn test_encode_record() -> anyhow::Result<()> {
        let record_type = Type::record([("x", Type::U32), ("y", Type::U32)])
            .context("failed to create record type")?;
        let record_value = Value::make_record(
            &record_type,
            [("x", Value::make_u32(10)), ("y", Value::make_u32(20))],
        )?;

        let mut encoder = ValueEncoder::new(&record_type);
        let mut buf = BytesMut::default();
        encoder.encode(&record_value, &mut buf)?;

        // Record with x=10, y=20 (LEB128)
        assert_eq!(buf.as_ref(), &[10, 20]);
        Ok(())
    }

    // ====== Tuple Tests ======

    #[test]
    fn test_encode_tuple() -> anyhow::Result<()> {
        let tuple_type =
            Type::tuple([Type::U32, Type::BOOL]).context("failed to create tuple type")?;
        let tuple_value =
            Value::make_tuple(&tuple_type, [Value::make_u32(42), Value::make_bool(true)])?;

        let mut encoder = ValueEncoder::new(&tuple_type);
        let mut buf = BytesMut::default();
        encoder.encode(&tuple_value, &mut buf)?;

        // Tuple with (42, true)
        assert_eq!(buf.as_ref(), &[42, 1]);
        Ok(())
    }

    // ====== Variant Tests ======

    #[test]
    fn test_encode_variant_no_payload() -> anyhow::Result<()> {
        let variant_type = Type::variant([("none", None), ("some", Some(Type::U32))])
            .context("failed to create variant type")?;
        let variant_value = Value::make_variant(&variant_type, "none", None)?;

        let mut encoder = ValueEncoder::new(&variant_type);
        let mut buf = BytesMut::default();
        encoder.encode(&variant_value, &mut buf)?;

        // Variant "none" (discriminant 0, no payload)
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    #[test]
    fn test_encode_variant_with_payload() -> anyhow::Result<()> {
        let variant_type = Type::variant([("none", None), ("some", Some(Type::U32))])
            .context("failed to create variant type")?;
        let variant_value = Value::make_variant(&variant_type, "some", Some(Value::make_u32(42)))?;

        let mut encoder = ValueEncoder::new(&variant_type);
        let mut buf = BytesMut::default();
        encoder.encode(&variant_value, &mut buf)?;

        // Variant "some" with payload 42 (discriminant 1, payload 42)
        assert_eq!(buf.as_ref(), &[1, 42]);
        Ok(())
    }

    // ====== Enum Tests ======

    #[test]
    fn test_encode_enum() -> anyhow::Result<()> {
        let enum_type = Type::enum_ty(["red", "blue"]).expect("enum type should be valid");
        let enum_value = Value::make_enum(&enum_type, "red")?;

        let mut encoder = ValueEncoder::new(&enum_type);
        let mut buf = BytesMut::default();
        encoder.encode(&enum_value, &mut buf)?;

        // First enum case should encode as discriminant 0
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    // ====== Option Tests ======

    #[test]
    fn test_encode_option_none() -> anyhow::Result<()> {
        let inner_type = Type::U32;
        let option_type = Type::option(inner_type);
        let option_value = Value::make_option(&option_type, None)?;

        let mut encoder = ValueEncoder::new(&option_type);
        let mut buf = BytesMut::default();
        encoder.encode(&option_value, &mut buf)?;

        // None is encoded as 0
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    #[test]
    fn test_encode_option_some() -> anyhow::Result<()> {
        let inner_type = Type::U32;
        let option_type = Type::option(inner_type.clone());
        let inner_value = Value::make_u32(42);
        let option_value = Value::make_option(&option_type, Some(inner_value))?;

        let mut encoder = ValueEncoder::new(&option_type);
        let mut buf = BytesMut::default();
        encoder.encode(&option_value, &mut buf)?;

        // Some(42) is encoded as 1 (discriminant) + 42 (value)
        assert_eq!(buf.as_ref(), &[1, 42]);
        Ok(())
    }

    // ====== Result Tests ======

    #[test]
    fn test_encode_result_ok_with_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), Some(Type::STRING));
        let result_value = Value::make_result(&result_type, Ok(Some(Value::make_u32(42))))?;

        let mut encoder = ValueEncoder::new(&result_type);
        let mut buf = BytesMut::default();
        encoder.encode(&result_value, &mut buf)?;

        // Ok(42): discriminant 0, value 42
        assert_eq!(buf.as_ref(), &[0, 42]);
        Ok(())
    }

    #[test]
    fn test_encode_result_ok_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(None, Some(Type::STRING));
        let result_value = Value::make_result(&result_type, Ok(None))?;

        let mut encoder = ValueEncoder::new(&result_type);
        let mut buf = BytesMut::default();
        encoder.encode(&result_value, &mut buf)?;

        // Ok(none): discriminant 0
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    #[test]
    fn test_encode_result_err_with_value() -> anyhow::Result<()> {
        use std::borrow::Cow;
        let result_type = Type::result(Some(Type::U32), Some(Type::STRING));
        let result_value = Value::make_result(
            &result_type,
            Err(Some(Value::make_string(Cow::Borrowed("fail")))),
        )?;

        let mut encoder = ValueEncoder::new(&result_type);
        let mut buf = BytesMut::default();
        encoder.encode(&result_value, &mut buf)?;

        // Err("fail"): discriminant 1, length 4, "fail"
        assert_eq!(buf.as_ref(), &[1, 4, b'f', b'a', b'i', b'l']);
        Ok(())
    }

    #[test]
    fn test_encode_result_err_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), None);
        let result_value = Value::make_result(&result_type, Err(None))?;

        let mut encoder = ValueEncoder::new(&result_type);
        let mut buf = BytesMut::default();
        encoder.encode(&result_value, &mut buf)?;

        // Err(none): discriminant 1
        assert_eq!(buf.as_ref(), &[1]);
        Ok(())
    }

    // ====== Flags Tests ======

    #[test]
    fn test_encode_flags_empty() -> anyhow::Result<()> {
        let flags_type =
            Type::flags(["read", "write", "execute"]).expect("flags type should be valid");
        let flags_value = Value::make_flags(&flags_type, [])?;

        let mut encoder = ValueEncoder::new(&flags_type);
        let mut buf = BytesMut::default();
        encoder.encode(&flags_value, &mut buf)?;

        // No flags set: 0b00000000 = 0
        assert_eq!(buf.as_ref(), &[0]);
        Ok(())
    }

    #[test]
    fn test_encode_flags_single_byte() -> anyhow::Result<()> {
        let flags_type =
            Type::flags(["read", "write", "execute"]).expect("flags type should be valid");
        let flags_value = Value::make_flags(&flags_type, ["read", "write"])?;

        let mut encoder = ValueEncoder::new(&flags_type);
        let mut buf = BytesMut::default();
        encoder.encode(&flags_value, &mut buf)?;

        // read=bit0, write=bit1 -> 0b00000011 = 3
        assert_eq!(buf.as_ref(), &[0b00000011]);
        Ok(())
    }

    #[test]
    fn test_encode_flags_two_bytes() -> anyhow::Result<()> {
        let flags_type = Type::flags([
            "f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11",
        ])
        .context("failed to create flags type")?;
        let flags_value = Value::make_flags(&flags_type, ["f0", "f9"])?;

        let mut encoder = ValueEncoder::new(&flags_type);
        let mut buf = BytesMut::default();
        encoder.encode(&flags_value, &mut buf)?;

        // f0 and f9 set: bit 0 in byte 0, bit 1 in byte 1
        // byte 0: 0b00000001 = 0x01
        // byte 1: 0b00000010 = 0x02
        assert_eq!(buf.as_ref(), &[0x01, 0x02]);
        Ok(())
    }

    #[test]
    fn test_encode_flags_three_bytes() -> anyhow::Result<()> {
        let flags_type = Type::flags([
            "f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13",
            "f14", "f15", "f16", "f17", "f18", "f19",
        ])
        .context("failed to create flags type")?;
        let flags_value = Value::make_flags(&flags_type, ["f0", "f8", "f16"])?;

        let mut encoder = ValueEncoder::new(&flags_type);
        let mut buf = BytesMut::default();
        encoder.encode(&flags_value, &mut buf)?;

        // f0, f8, f16 set: bit 0 in byte 0, bit 0 in byte 1, bit 0 in byte 2
        assert_eq!(buf.as_ref(), &[0x01, 0x01, 0x01]);
        Ok(())
    }
}
