//! Value decoding

use core::mem;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use std::borrow::Cow;

use tokio::io::{AsyncRead, AsyncReadExt as _};
use wasm_tokio::{
    AsyncReadCore as _, AsyncReadLeb128 as _, AsyncReadUtf8 as _, DEFAULT_MAX_INITIAL_CAPACITY,
    cm::AsyncReadValue as _,
};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue};

/// Read a value of type `ty` from `r`
///
/// # Errors
///
/// - [`UnexpectedEof`](std::io::ErrorKind::UnexpectedEof) if `r` ends before the value is complete
/// - [`InvalidData`](std::io::ErrorKind::InvalidData) if the encoding is malformed
/// - Any error returned by `r`
#[allow(clippy::too_many_lines)]
pub async fn read_value<V, R>(r: &mut R, ty: &V::Type) -> std::io::Result<V>
where
    V: WasmValue,
    R: AsyncRead + Unpin,
{
    match ty.kind() {
        WasmTypeKind::Bool => Ok(V::make_bool(r.read_bool().await?)),
        WasmTypeKind::S8 => Ok(V::make_s8(r.read_i8().await?)),
        WasmTypeKind::U8 => Ok(V::make_u8(r.read_u8().await?)),
        WasmTypeKind::S16 => Ok(V::make_s16(r.read_i16_leb128().await?)),
        WasmTypeKind::U16 => Ok(V::make_u16(r.read_u16_leb128().await?)),
        WasmTypeKind::S32 => Ok(V::make_s32(r.read_i32_leb128().await?)),
        WasmTypeKind::U32 => Ok(V::make_u32(r.read_u32_leb128().await?)),
        WasmTypeKind::S64 => Ok(V::make_s64(r.read_i64_leb128().await?)),
        WasmTypeKind::U64 => Ok(V::make_u64(r.read_u64_leb128().await?)),
        WasmTypeKind::F32 => Ok(V::make_f32(r.read_f32_le().await?)),
        WasmTypeKind::F64 => Ok(V::make_f64(r.read_f64_le().await?)),
        WasmTypeKind::Char => Ok(V::make_char(r.read_char_utf8().await?)),
        WasmTypeKind::String => {
            let mut s = String::default();
            r.read_core_name(&mut s).await?;
            Ok(V::make_string(Cow::Owned(s)))
        }
        WasmTypeKind::List => {
            let n = r.read_u32_leb128().await?;
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            let element_type = ty
                .list_element_type()
                .ok_or_else(|| io_error("list type missing element type"))?;
            let mut elements = Vec::with_capacity(
                n.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<V>().max(1)),
            );
            for _ in 0..n {
                elements.push(Box::pin(read_value(r, &element_type)).await?);
            }
            V::make_list(ty, elements).map_err(io_error)
        }
        WasmTypeKind::Record => {
            let (names, types): (Vec<_>, Vec<_>) = ty.record_fields().unzip();
            let mut values = Vec::with_capacity(types.len());
            for ty in types {
                values.push(Box::pin(read_value(r, &ty)).await?);
            }
            V::make_record(ty, names.iter().map(AsRef::as_ref).zip(values)).map_err(io_error)
        }
        WasmTypeKind::Tuple => {
            let element_types: Vec<_> = ty.tuple_element_types().collect();
            let mut elements = Vec::with_capacity(element_types.len());
            for element_type in element_types {
                elements.push(Box::pin(read_value(r, &element_type)).await?);
            }
            V::make_tuple(ty, elements).map_err(io_error)
        }
        WasmTypeKind::Variant => {
            let discriminant = r.read_u32_leb128().await?;
            let cases: Vec<_> = ty.variant_cases().collect();
            let (name, payload_type) = usize::try_from(discriminant)
                .ok()
                .and_then(|i| cases.get(i))
                .ok_or_else(|| io_error(format!("unknown variant discriminant: {discriminant}")))?;
            let payload = match payload_type {
                Some(payload_type) => Some(Box::pin(read_value(r, payload_type)).await?),
                None => None,
            };
            V::make_variant(ty, name, payload).map_err(io_error)
        }
        WasmTypeKind::Enum => {
            let discriminant = r.read_u32_leb128().await?;
            let names: Vec<_> = ty.enum_cases().collect();
            let name = usize::try_from(discriminant)
                .ok()
                .and_then(|i| names.get(i))
                .ok_or_else(|| io_error(format!("unknown enum discriminant: {discriminant}")))?;
            V::make_enum(ty, name).map_err(io_error)
        }
        WasmTypeKind::Option => {
            let v = if r.read_option_status().await? {
                let ty = ty
                    .option_some_type()
                    .ok_or_else(|| io_error("option type missing some type"))?;
                Some(Box::pin(read_value(r, &ty)).await?)
            } else {
                None
            };
            V::make_option(ty, v).map_err(io_error)
        }
        WasmTypeKind::Result => {
            let ok = r.read_result_status().await?;
            let (ok_type, err_type) = ty
                .result_types()
                .ok_or_else(|| io_error("result type missing ok/err types"))?;
            let payload_type = if ok { ok_type } else { err_type };
            let payload = match payload_type {
                Some(ty) => Some(Box::pin(read_value(r, &ty)).await?),
                None => None,
            };
            V::make_result(ty, if ok { Ok(payload) } else { Err(payload) }).map_err(io_error)
        }
        WasmTypeKind::Flags => {
            let names: Vec<_> = ty.flags_names().collect();
            let mut buf = vec![0u8; names.len().div_ceil(8).max(1)];
            r.read_exact(&mut buf).await?;
            let set = names
                .iter()
                .enumerate()
                .filter(|(i, _)| buf[i / 8] & (1 << (i % 8)) != 0)
                .map(|(_, name)| name.as_ref());
            V::make_flags(ty, set).map_err(io_error)
        }
        kind => Err(io_error(format!("unsupported value type: {kind:?}"))),
    }
}

/// Decode a value of type `ty` from `buf`
///
/// # Errors
///
/// Returns the errors of [`read_value`] and [`InvalidData`](std::io::ErrorKind::InvalidData) if
/// `buf` is not fully consumed.
pub fn decode_value<V: WasmValue>(mut buf: &[u8], ty: &V::Type) -> std::io::Result<V> {
    let Poll::Ready(v) =
        pin!(read_value(&mut buf, ty)).poll(&mut Context::from_waker(Waker::noop()))
    else {
        return Err(std::io::ErrorKind::WouldBlock.into());
    };
    let v = v?;
    if !buf.is_empty() {
        return Err(io_error("buffer contains trailing bytes"));
    }
    Ok(v)
}

fn io_error(err: impl core::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use wasm_wave::value::{Type, Value};

    fn decode_sync(ty: &Type, data: &[u8]) -> std::io::Result<Value> {
        decode_value(data, ty)
    }

    // Bool types
    #[test]
    fn test_decode_bool_true() -> anyhow::Result<()> {
        let value = decode_sync(&Type::BOOL, &[1u8])?;
        assert!(value.unwrap_bool());
        Ok(())
    }

    #[test]
    fn test_decode_bool_false() -> anyhow::Result<()> {
        let value = decode_sync(&Type::BOOL, &[0u8])?;
        assert!(!value.unwrap_bool());
        Ok(())
    }

    // Integer types
    #[test]
    fn test_decode_s8_positive() -> anyhow::Result<()> {
        let value = decode_sync(&Type::S8, &[42u8])?;
        assert_eq!(value.unwrap_s8(), 42);
        Ok(())
    }

    #[test]
    fn test_decode_s8_negative() -> anyhow::Result<()> {
        let value = decode_sync(&Type::S8, &[(-42i8) as u8])?;
        assert_eq!(value.unwrap_s8(), -42);
        Ok(())
    }

    #[test]
    fn test_decode_u8() -> anyhow::Result<()> {
        let value = decode_sync(&Type::U8, &[255u8])?;
        assert_eq!(value.unwrap_u8(), 255);
        Ok(())
    }

    #[test]
    fn test_decode_s16() -> anyhow::Result<()> {
        let value = decode_sync(&Type::S16, &[0xE8, 0x07])?;
        assert_eq!(value.unwrap_s16(), 1000);
        Ok(())
    }

    #[test]
    fn test_decode_u16() -> anyhow::Result<()> {
        let value = decode_sync(&Type::U16, &[0xE8, 0x07])?;
        assert_eq!(value.unwrap_u16(), 1000);
        Ok(())
    }

    #[test]
    fn test_decode_s32() -> anyhow::Result<()> {
        let value = decode_sync(&Type::S32, &[0xA0, 0x8D, 0x06])?;
        assert_eq!(value.unwrap_s32(), 100000);
        Ok(())
    }

    #[test]
    fn test_decode_u32() -> anyhow::Result<()> {
        let value = decode_sync(&Type::U32, &[42u8])?;
        assert_eq!(value.unwrap_u32(), 42);
        Ok(())
    }

    #[test]
    fn test_decode_s64() -> anyhow::Result<()> {
        let value = decode_sync(&Type::S64, &[0x7F])?;
        assert_eq!(value.unwrap_s64(), -1);
        Ok(())
    }

    #[test]
    fn test_decode_u64() -> anyhow::Result<()> {
        let value = decode_sync(&Type::U64, &[0xB9, 0x60])?;
        assert_eq!(value.unwrap_u64(), 12345);
        Ok(())
    }

    // Float types
    #[test]
    fn test_decode_f32() -> anyhow::Result<()> {
        let value = decode_sync(&Type::F32, &1.5f32.to_le_bytes())?;
        assert_eq!(value.unwrap_f32(), 1.5);
        Ok(())
    }

    #[test]
    fn test_decode_f64() -> anyhow::Result<()> {
        let value = decode_sync(&Type::F64, &2.5f64.to_le_bytes())?;
        assert_eq!(value.unwrap_f64(), 2.5);
        Ok(())
    }

    // Char types
    #[test]
    fn test_decode_char() -> anyhow::Result<()> {
        let value = decode_sync(&Type::CHAR, &[0x41])?;
        assert_eq!(value.unwrap_char(), 'A');
        Ok(())
    }

    #[test]
    fn test_decode_char_unicode() -> anyhow::Result<()> {
        let value = decode_sync(&Type::CHAR, &[0xF0, 0x9F, 0x98, 0x80])?;
        assert_eq!(value.unwrap_char(), '😀');
        Ok(())
    }

    // String types
    #[test]
    fn test_decode_string() -> anyhow::Result<()> {
        let value = decode_sync(&Type::STRING, &[5, b'h', b'e', b'l', b'l', b'o'])?;
        assert_eq!(value.unwrap_string().as_ref(), "hello");
        Ok(())
    }

    // List types
    #[test]
    fn test_decode_list_empty() -> anyhow::Result<()> {
        let list_type = Type::list(Type::U32);
        let value = decode_sync(&list_type, &[0u8])?;
        let list: Vec<_> = value.unwrap_list().collect();
        assert_eq!(list.len(), 0);
        Ok(())
    }

    #[test]
    fn test_decode_list_u32() -> anyhow::Result<()> {
        let list_type = Type::list(Type::U32);
        let value = decode_sync(&list_type, &[3, 1, 2, 3])?;
        let list: Vec<_> = value.unwrap_list().collect();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].unwrap_u32(), 1);
        assert_eq!(list[1].unwrap_u32(), 2);
        assert_eq!(list[2].unwrap_u32(), 3);
        Ok(())
    }

    // Record types
    #[test]
    fn test_decode_record() -> anyhow::Result<()> {
        use anyhow::Context as _;
        let record_type = Type::record([("x", Type::U32), ("y", Type::U32)])
            .context("failed to create record type")?;
        let value = decode_sync(&record_type, &[10, 20])?;
        let fields: Vec<_> = value.unwrap_record().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0.as_ref(), "x");
        assert_eq!(fields[0].1.unwrap_u32(), 10);
        assert_eq!(fields[1].0.as_ref(), "y");
        assert_eq!(fields[1].1.unwrap_u32(), 20);
        Ok(())
    }

    // Tuple types
    #[test]
    fn test_decode_tuple() -> anyhow::Result<()> {
        use anyhow::Context as _;
        let tuple_type =
            Type::tuple([Type::U32, Type::BOOL]).context("failed to create tuple type")?;
        let value = decode_sync(&tuple_type, &[42, 1])?;
        let elements: Vec<_> = value.unwrap_tuple().collect();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].unwrap_u32(), 42);
        assert!(elements[1].unwrap_bool());
        Ok(())
    }

    // Variant types
    #[test]
    fn test_decode_variant_no_payload() -> anyhow::Result<()> {
        use anyhow::Context as _;
        let variant_type = Type::variant([("none", None), ("some", Some(Type::U32))])
            .context("failed to create variant type")?;
        let value = decode_sync(&variant_type, &[0])?;
        let (case, payload) = value.unwrap_variant();
        assert_eq!(case.as_ref(), "none");
        assert!(payload.is_none());
        Ok(())
    }

    #[test]
    fn test_decode_variant_with_payload() -> anyhow::Result<()> {
        use anyhow::Context as _;
        let variant_type = Type::variant([("none", None), ("some", Some(Type::U32))])
            .context("failed to create variant type")?;
        let value = decode_sync(&variant_type, &[1, 42])?;
        let (case, payload) = value.unwrap_variant();
        assert_eq!(case.as_ref(), "some");
        assert_eq!(payload.unwrap().unwrap_u32(), 42);
        Ok(())
    }

    // Enum types
    #[test]
    fn test_decode_enum() -> anyhow::Result<()> {
        use anyhow::Context as _;
        let enum_type =
            Type::enum_ty(["red", "green", "blue"]).context("failed to create enum type")?;
        let value = decode_sync(&enum_type, &[1])?;
        assert_eq!(value.unwrap_enum().as_ref(), "green");
        Ok(())
    }

    // Option types
    #[test]
    fn test_decode_option_none() -> anyhow::Result<()> {
        let option_type = Type::option(Type::U32);
        let value = decode_sync(&option_type, &[0u8])?;
        assert!(value.unwrap_option().is_none());
        Ok(())
    }

    #[test]
    fn test_decode_option_some() -> anyhow::Result<()> {
        let option_type = Type::option(Type::U32);
        let value = decode_sync(&option_type, &[1, 42])?;
        assert_eq!(value.unwrap_option().unwrap().unwrap_u32(), 42);
        Ok(())
    }

    // Result types
    #[test]
    fn test_decode_result_ok_with_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), Some(Type::STRING));
        let value = decode_sync(&result_type, &[0, 42])?;
        let result = value.unwrap_result();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap().unwrap_u32(), 42);
        Ok(())
    }

    #[test]
    fn test_decode_result_ok_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(None, Some(Type::STRING));
        let value = decode_sync(&result_type, &[0])?;
        let result = value.unwrap_result();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        Ok(())
    }

    #[test]
    fn test_decode_result_err_with_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), Some(Type::STRING));
        let value = decode_sync(&result_type, &[1, 4, b'f', b'a', b'i', b'l'])?;
        let result = value.unwrap_result();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().unwrap().unwrap_string().as_ref(),
            "fail"
        );
        Ok(())
    }

    #[test]
    fn test_decode_result_err_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), None);
        let value = decode_sync(&result_type, &[1])?;
        let result = value.unwrap_result();
        assert!(result.is_err());
        assert!(result.unwrap_err().is_none());
        Ok(())
    }

    // Flags types
    #[test]
    fn test_decode_flags_none_set() -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::collections::HashSet;
        let flags_type = Type::flags(["read", "write", "execute", "append"])
            .context("failed to create flags type")?;
        let value = decode_sync(&flags_type, &[0x00])?;
        let flags: HashSet<_> = value.unwrap_flags().map(|s| s.into_owned()).collect();
        assert_eq!(flags.len(), 0);
        Ok(())
    }

    #[test]
    fn test_decode_flags_single_byte() -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::collections::HashSet;
        let flags_type = Type::flags(["read", "write", "execute", "append"])
            .context("failed to create flags type")?;
        let value = decode_sync(&flags_type, &[0x05])?;
        let flags: HashSet<_> = value.unwrap_flags().map(|s| s.into_owned()).collect();
        assert_eq!(flags.len(), 2);
        assert!(flags.contains("read"));
        assert!(flags.contains("execute"));
        Ok(())
    }

    #[test]
    fn test_decode_flags_two_bytes() -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::collections::HashSet;
        let flags_type = Type::flags([
            "f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11",
        ])
        .context("failed to create flags type")?;
        let value = decode_sync(&flags_type, &[0x01, 0x02])?;
        let flags: HashSet<_> = value.unwrap_flags().map(|s| s.into_owned()).collect();
        assert_eq!(flags.len(), 2);
        assert!(flags.contains("f0"));
        assert!(flags.contains("f9"));
        Ok(())
    }

    #[test]
    fn test_decode_flags_three_bytes() -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::collections::HashSet;
        let flags_type = Type::flags([
            "f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13",
            "f14", "f15", "f16", "f17", "f18", "f19",
        ])
        .context("failed to create flags type")?;
        let value = decode_sync(&flags_type, &[0x01, 0x01, 0x01])?;
        let flags: HashSet<_> = value.unwrap_flags().map(|s| s.into_owned()).collect();
        assert_eq!(flags.len(), 3);
        assert!(flags.contains("f0"));
        assert!(flags.contains("f8"));
        assert!(flags.contains("f16"));
        Ok(())
    }

    // Error cases
    #[test]
    fn test_decode_option_invalid_status() {
        let err = decode_sync(&Type::option(Type::U32), &[2, 42]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_decode_result_invalid_status() {
        let result_type = Type::result(Some(Type::U32), Some(Type::U32));
        let err = decode_sync(&result_type, &[2, 42]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_decode_list_truncated() {
        let err = decode_sync(&Type::list(Type::U8), &[0xff, 0xff, 0xff, 0xff, 0x0f]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn test_decode_trailing_bytes() {
        let err = decode_sync(&Type::U8, &[42, 99, 100]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_incomplete_data_eof() {
        let result = decode_sync(&Type::U32, &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
