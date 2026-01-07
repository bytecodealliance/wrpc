//! Async value decoder for wasm-wave types
//!
//! This module provides `read_value`, which mirrors `wrpc_runtime_wasmtime::codec::read_value`
//! but works with `wasm_wave::value::Value` instead of `wasmtime::component::Val`.
//!
//! ## Design
//!
//! This decoder uses async reads that block until complete data is available
//!
//! ## Usage
//!
//! ```no_run
//! use wrpc_wave::read_value;
//! use wasm_wave::value::Type;
//! use tokio::io::AsyncRead;
//!
//! # async fn example<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<()> {
//! let mut pinned = std::pin::pin!(reader);
//! let value = read_value(&mut pinned, &wasm_wave::value::Type::U32).await?;
//! # Ok(())
//! # }
//! ```

use std::borrow::Cow;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncReadExt as _};
use wasm_tokio::{
    cm::AsyncReadValue as _, AsyncReadCore as _, AsyncReadLeb128 as _, AsyncReadUtf8 as _,
};
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue as _};

/// Read a wasm-wave `Value` from an async reader
///
/// This mirrors `wrpc_runtime_wasmtime::codec::read_value` but without the
/// `Index<R>` trait bound and `path` parameter, since wasm-wave doesn't support resources.
///
/// # Errors
///
/// - `UnexpectedEof` - Not enough data in stream
/// - `InvalidData` - Malformed encoding (invalid UTF-8, discriminant, etc.)
/// - Other `io::Error` from underlying reader
#[allow(clippy::too_many_lines)]
pub async fn read_value<R>(r: &mut Pin<&mut R>, ty: &Type) -> std::io::Result<Value>
where
    R: AsyncRead + Unpin,
{
    match ty.kind() {
        WasmTypeKind::Bool => {
            let v = r.read_bool().await?;
            Ok(Value::make_bool(v))
        }
        WasmTypeKind::S8 => {
            let v = r.read_i8().await?;
            Ok(Value::make_s8(v))
        }
        WasmTypeKind::U8 => {
            let v = r.read_u8().await?;
            Ok(Value::make_u8(v))
        }
        WasmTypeKind::S16 => {
            let v = r.read_i16_leb128().await?;
            Ok(Value::make_s16(v))
        }
        WasmTypeKind::U16 => {
            let v = r.read_u16_leb128().await?;
            Ok(Value::make_u16(v))
        }
        WasmTypeKind::S32 => {
            let v = r.read_i32_leb128().await?;
            Ok(Value::make_s32(v))
        }
        WasmTypeKind::U32 => {
            let v = r.read_u32_leb128().await?;
            Ok(Value::make_u32(v))
        }
        WasmTypeKind::S64 => {
            let v = r.read_i64_leb128().await?;
            Ok(Value::make_s64(v))
        }
        WasmTypeKind::U64 => {
            let v = r.read_u64_leb128().await?;
            Ok(Value::make_u64(v))
        }
        WasmTypeKind::F32 => {
            let v = r.read_f32_le().await?;
            Ok(Value::make_f32(v))
        }
        WasmTypeKind::F64 => {
            let v = r.read_f64_le().await?;
            Ok(Value::make_f64(v))
        }
        WasmTypeKind::Char => {
            let v = r.read_char_utf8().await?;
            Ok(Value::make_char(v))
        }
        WasmTypeKind::String => {
            let mut s = String::default();
            r.read_core_name(&mut s).await?;
            Ok(Value::make_string(Cow::Owned(s)))
        }
        WasmTypeKind::List => {
            let n = r.read_u32_leb128().await?;
            let n = n.try_into().unwrap_or(usize::MAX);
            let element_type = ty
                .list_element_type()
                .ok_or_else(|| io_error("list type missing element type"))?;

            let mut elements = Vec::with_capacity(n);
            for _ in 0..n {
                let element = Box::pin(read_value(r, &element_type)).await?;
                elements.push(element);
            }

            Value::make_list(ty, elements).map_err(io_error)
        }
        WasmTypeKind::Record => {
            let field_types: Vec<_> = ty.record_fields().collect();
            let mut field_names = Vec::with_capacity(field_types.len());
            let mut field_values = Vec::with_capacity(field_types.len());

            for (name, field_type) in field_types {
                let value = Box::pin(read_value(r, &field_type)).await?;
                field_names.push(name.to_string());
                field_values.push(value);
            }

            // Create the fields iterator from owned strings
            let fields = field_names
                .iter()
                .map(|s| s.as_str())
                .zip(field_values.into_iter());

            Value::make_record(ty, fields).map_err(io_error)
        }
        WasmTypeKind::Tuple => {
            let element_types: Vec<_> = ty.tuple_element_types().collect();
            let mut elements = Vec::with_capacity(element_types.len());

            for element_type in element_types {
                let element = Box::pin(read_value(r, &element_type)).await?;
                elements.push(element);
            }

            Value::make_tuple(ty, elements).map_err(io_error)
        }
        WasmTypeKind::Variant => {
            let discriminant = r.read_u32_leb128().await?;
            let cases: Vec<_> = ty.variant_cases().collect();

            let discriminant_idx: usize = discriminant
                .try_into()
                .map_err(|_| io_error("variant discriminant too large"))?;

            let (case_name, payload_type) = cases.get(discriminant_idx).ok_or_else(|| {
                io_error(format!("unknown variant discriminant: {}", discriminant))
            })?;

            let payload = if let Some(payload_type) = payload_type {
                let value = Box::pin(read_value(r, &payload_type)).await?;
                Some(value)
            } else {
                None
            };

            Value::make_variant(ty, case_name, payload).map_err(io_error)
        }
        WasmTypeKind::Enum => {
            let discriminant = r.read_u32_leb128().await?;
            let names: Vec<_> = ty.enum_cases().collect();

            let discriminant_idx: usize = discriminant
                .try_into()
                .map_err(|_| io_error("enum discriminant too large"))?;

            let name = names
                .get(discriminant_idx)
                .ok_or_else(|| io_error(format!("unknown enum discriminant: {}", discriminant)))?;

            Value::make_enum(ty, name).map_err(io_error)
        }
        WasmTypeKind::Option => {
            let ok = r.read_u8().await?;

            if ok != 0 {
                let inner_type = ty
                    .option_some_type()
                    .ok_or_else(|| io_error("option type missing some type"))?;
                let value = Box::pin(read_value(r, &inner_type)).await?;
                Value::make_option(ty, Some(value)).map_err(io_error)
            } else {
                Value::make_option(ty, None).map_err(io_error)
            }
        }
        WasmTypeKind::Result => {
            let ok = r.read_u8().await?;
            let (ok_type, err_type) = ty
                .result_types()
                .ok_or_else(|| io_error("result type missing ok/err types"))?;

            if ok == 0 {
                // Ok variant
                if let Some(ok_ty) = ok_type {
                    let value = Box::pin(read_value(r, &ok_ty)).await?;
                    Value::make_result(ty, Ok(Some(value)))
                } else {
                    Value::make_result(ty, Ok(None))
                }
            } else if ok == 1 {
                // Err variant
                if let Some(err_ty) = err_type {
                    let value = Box::pin(read_value(r, &err_ty)).await?;
                    Value::make_result(ty, Err(Some(value)))
                } else {
                    Value::make_result(ty, Err(None))
                }
            } else {
                return Err(io_error(format!("invalid result discriminant: {}", ok)));
            }
            .map_err(io_error)
        }
        WasmTypeKind::Flags => {
            let names: Vec<_> = ty.flags_names().collect();
            let byte_count = (names.len() + 7) / 8; // Ceiling division

            let mut buf = vec![0u8; byte_count];
            r.read_exact(&mut buf).await?;

            let mut flag_names = Vec::new();
            for (i, name) in names.iter().enumerate() {
                if buf[i / 8] & (1 << (i % 8)) != 0 {
                    flag_names.push(name.as_ref());
                }
            }

            Value::make_flags(ty, flag_names).map_err(io_error)
        }
        WasmTypeKind::Unsupported => Err(io_error("unsupported value type")),
        _ => Err(io_error(format!("unsupported value type: {:?}", ty.kind()))),
    }
}

// Helper to convert any error to io::Error with InvalidData kind
fn io_error(err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_wave::value::Type;

    // Test helper - uses futures::executor (no tokio runtime needed in tests)
    fn decode_sync(ty: &Type, data: &[u8]) -> std::io::Result<Value> {
        let mut cursor = std::io::Cursor::new(data);
        futures::executor::block_on(async {
            let mut pinned = std::pin::pin!(&mut cursor);
            read_value(&mut pinned, ty).await
        })
    }

    // Bool types
    #[test]
    fn test_decode_bool_true() -> anyhow::Result<()> {
        let value = decode_sync(&Type::BOOL, &[1u8])?;
        assert_eq!(value.unwrap_bool(), true);
        Ok(())
    }

    #[test]
    fn test_decode_bool_false() -> anyhow::Result<()> {
        let value = decode_sync(&Type::BOOL, &[0u8])?;
        assert_eq!(value.unwrap_bool(), false);
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
        let value = decode_sync(&Type::F32, &3.14f32.to_le_bytes())?;
        assert!((value.unwrap_f32() - 3.14).abs() < 0.01);
        Ok(())
    }

    #[test]
    fn test_decode_f64() -> anyhow::Result<()> {
        let value = decode_sync(&Type::F64, &3.14159265359f64.to_le_bytes())?;
        assert!((value.unwrap_f64() - 3.14159265359).abs() < 0.00000001);
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
        assert_eq!(elements[1].unwrap_bool(), true);
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
    fn test_incomplete_data_eof() {
        let result = decode_sync(&Type::U32, &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
