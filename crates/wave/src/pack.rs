//! Typed value wrapper for use with wrpc-pack
//!
//! This module provides `WasmTypedValue` which bundles a value and its type together,
//! allowing it to be used with the `wrpc_pack::pack` function.

use bytes::BytesMut;
use tokio_util::codec::Encoder;
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::{WasmType, WasmValue};
use wrpc_pack::NoopStream;
use wrpc_transport::Deferred;

use crate::core::encode::WaveEncoder;

/// Encoder that implements the necessary traits for use with `pack`.
///
/// Unlike `WaveEncoder` which stores a reference to the type,
/// this encoder is stateless and implements `Default`, which is required by the
/// `wrpc_transport::Encode` trait.
#[derive(Debug, Default)]
pub struct TypedWaveEncoder;

impl Deferred<NoopStream> for TypedWaveEncoder {
    fn take_deferred(&mut self) -> Option<wrpc_transport::DeferredFn<NoopStream>> {
        // NoopStream doesn't support async operations, so we never have deferred values
        None
    }
}

// Generic implementation for any WasmValue + WasmType pair (owned)
impl<V, T> Encoder<WasmTypedValue<V, T>> for TypedWaveEncoder
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = anyhow::Error;

    fn encode(&mut self, v: WasmTypedValue<V, T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut encoder = WaveEncoder::new(&v.1);
        encoder.encode(&v.0, dst)
    }
}

// Generic implementation for any WasmValue + WasmType pair (borrowed)
impl<V, T> Encoder<&WasmTypedValue<V, T>> for TypedWaveEncoder
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = anyhow::Error;

    fn encode(&mut self, v: &WasmTypedValue<V, T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut encoder = WaveEncoder::new(&v.1);
        encoder.encode(&v.0, dst)
    }
}

/// This wrapper contains both a value implementing [`WasmValue`] and its type implementing [`WasmType`].
/// Both are required for proper encoding of complex types like variants, enums, and flags.
///
/// The generic parameters allow this wrapper to work with any implementations of the traits,
/// such as `wasm_wave::value::Value` and `wasm_wave::value::Type`, or `wasmtime::component::Val`.
///
/// # Examples
///
/// Using with the default `Value` and `Type`:
/// ```
/// use wrpc_wave::WasmTypedValue;
/// use wasm_wave::value::{Value, Type};
/// use wasm_wave::wasm::WasmValue;
///
/// let value = Value::make_u32(42);
/// let ty = Type::U32;
/// let typed_value = WasmTypedValue(value, ty);
/// ```
///
/// Due to default type parameters, this is equivalent to `WasmTypedValue<Value, Type>`.
#[derive(Debug, Clone)]
pub struct WasmTypedValue<V: WasmValue = Value, T: WasmType = Type>(pub V, pub T);

impl<V: WasmValue, T: WasmType> From<(V, T)> for WasmTypedValue<V, T> {
    fn from((value, ty): (V, T)) -> Self {
        WasmTypedValue(value, ty)
    }
}

impl<V: WasmValue, T: WasmType> From<WasmTypedValue<V, T>> for (V, T) {
    fn from(wrpc_value: WasmTypedValue<V, T>) -> Self {
        (wrpc_value.0, wrpc_value.1)
    }
}

// Generic implementations for wrpc_transport::Encode
impl<V, T> wrpc_transport::Encode<NoopStream> for WasmTypedValue<V, T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Encoder = TypedWaveEncoder;
}

impl<V, T> wrpc_transport::Encode<NoopStream> for &WasmTypedValue<V, T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Encoder = TypedWaveEncoder;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use wasm_wave::value::Value;
    use wrpc_pack::pack;

    #[test]
    fn test_pack_bool() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(true, &mut params_bytes_regular)?;

        let value = Value::make_bool(true);
        let ty = Type::BOOL;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for bool"
        );
        Ok(())
    }

    #[test]
    fn test_pack_s8() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(42i8, &mut params_bytes_regular)?;

        let value = Value::make_s8(42);
        let ty = Type::S8;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for s8"
        );
        Ok(())
    }

    #[test]
    fn test_pack_u8() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(42u8, &mut params_bytes_regular)?;

        let value = Value::make_u8(42);
        let ty = Type::U8;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for u8"
        );
        Ok(())
    }

    #[test]
    fn test_pack_s16() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(1234i16, &mut params_bytes_regular)?;

        let value = Value::make_s16(1234);
        let ty = Type::S16;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for s16"
        );
        Ok(())
    }

    #[test]
    fn test_pack_u16() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(1234u16, &mut params_bytes_regular)?;

        let value = Value::make_u16(1234);
        let ty = Type::U16;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for u16"
        );
        Ok(())
    }

    #[test]
    fn test_pack_s32() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(123456i32, &mut params_bytes_regular)?;

        let value = Value::make_s32(123456);
        let ty = Type::S32;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for s32"
        );
        Ok(())
    }

    #[test]
    fn test_pack_u32() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(123456u32, &mut params_bytes_regular)?;

        let value = Value::make_u32(123456);
        let ty = Type::U32;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for u32"
        );
        Ok(())
    }

    #[test]
    fn test_pack_s64() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(1234567890i64, &mut params_bytes_regular)?;

        let value = Value::make_s64(1234567890);
        let ty = Type::S64;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for s64"
        );
        Ok(())
    }

    #[test]
    fn test_pack_u64() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(1234567890u64, &mut params_bytes_regular)?;

        let value = Value::make_u64(1234567890);
        let ty = Type::U64;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for u64"
        );
        Ok(())
    }

    #[test]
    fn test_pack_f32() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(3.14f32, &mut params_bytes_regular)?;

        let value = Value::make_f32(3.14);
        let ty = Type::F32;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for f32"
        );
        Ok(())
    }

    #[test]
    fn test_pack_f64() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(3.14159f64, &mut params_bytes_regular)?;

        let value = Value::make_f64(3.14159);
        let ty = Type::F64;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for f64"
        );
        Ok(())
    }

    #[test]
    fn test_pack_char() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack('A', &mut params_bytes_regular)?;

        let value = Value::make_char('A');
        let ty = Type::CHAR;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for char"
        );
        Ok(())
    }

    #[test]
    fn test_pack_string() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack("hello", &mut params_bytes_regular)?;

        use std::borrow::Cow;
        let value = Value::make_string(Cow::Borrowed("hello"));
        let ty = Type::STRING;
        let wrpc_value = WasmTypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for string"
        );
        Ok(())
    }

    #[test]
    fn test_pack_list() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(vec![1u32, 2u32, 3u32], &mut params_bytes_regular)?;

        let element_type = Type::U32;
        let list_type = Type::list(element_type.clone());
        let values = vec![Value::make_u32(1), Value::make_u32(2), Value::make_u32(3)];
        let list_value = Value::make_list(&list_type, values)?;
        let wrpc_value = WasmTypedValue(list_value, list_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for list"
        );
        Ok(())
    }

    #[test]
    fn test_pack_record() -> anyhow::Result<()> {
        // Records are encoded as tuples (just values, no field names)
        // So we'll test with a record that has two u32 fields
        // This should encode the same as a tuple (u32, u32)
        let mut params_bytes_regular = BytesMut::new();
        pack((42u32, 100u32), &mut params_bytes_regular)?;

        let record_type = Type::record([("a", Type::U32), ("b", Type::U32)])
            .expect("record type should be valid");
        let record_value = Value::make_record(
            &record_type,
            [("a", Value::make_u32(42)), ("b", Value::make_u32(100))],
        )?;
        let wrpc_value = WasmTypedValue(record_value, record_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for record"
        );
        Ok(())
    }

    #[test]
    fn test_pack_tuple() -> anyhow::Result<()> {
        // Test 1: Pack (5,) using regular pack
        let mut params_bytes_regular = BytesMut::new();
        pack((5u32,), &mut params_bytes_regular)?;

        // Test 2: Pack a wasm-wave Value representing (5,) as a tuple
        let value_5 = Value::make_u32(5);
        let tuple_type = Type::tuple([Type::U32]).expect("tuple type should be valid");
        let tuple_value = Value::make_tuple(&tuple_type, [value_5])?;
        let wrpc_tuple_value = WasmTypedValue(tuple_value, tuple_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_tuple_value, &mut params_bytes_wave)?;

        // Test 3: Verify both produce the same bytes
        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes"
        );

        Ok(())
    }

    #[test]
    fn test_pack_variant() -> anyhow::Result<()> {
        // Test variant with payload
        // Variant encoding: discriminant index + optional payload
        // We'll create a variant with two cases: "none" (no payload) and "some" (with u32 payload)
        // Test the "some" case
        let variant_type = Type::variant([("none", None), ("some", Some(Type::U32))])
            .expect("variant type should be valid");

        // For comparison, we'll use Option<u32>::Some(42) which encodes similarly
        let mut params_bytes_regular = BytesMut::new();
        pack(Some(42u32), &mut params_bytes_regular)?;

        let variant_value = Value::make_variant(&variant_type, "some", Some(Value::make_u32(42)))?;
        let wrpc_value = WasmTypedValue(variant_value, variant_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        // Note: Variant encoding is similar to Option but uses discriminant index
        // The bytes should match for a simple case like this
        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for variant"
        );
        Ok(())
    }

    #[test]
    fn test_pack_enum() -> anyhow::Result<()> {
        // Enums encode as just the discriminant index
        // We'll create a simple enum with two cases: "red" and "blue"
        // Test encoding "red" (discriminant 0)
        let enum_type = Type::enum_ty(["red", "blue"]).expect("enum type should be valid");

        // For comparison, we can't directly compare with a Rust enum easily
        // So we'll just test that it encodes without error and produces some bytes
        let enum_value = Value::make_enum(&enum_type, "red")?;
        let wrpc_value = WasmTypedValue(enum_value, enum_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        // Enum should encode as a single byte (discriminant 0 for first case in small enum)
        assert!(
            !params_bytes_wave.is_empty(),
            "Enum should produce non-empty bytes"
        );
        // For a 2-case enum, it should use u8 encoding (1 byte for discriminant)
        assert_eq!(
            params_bytes_wave.len(),
            1,
            "Small enum should encode as 1 byte"
        );
        assert_eq!(
            params_bytes_wave[0], 0,
            "First enum case should encode as discriminant 0"
        );
        Ok(())
    }

    #[test]
    fn test_pack_option_none() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(Option::<u32>::None, &mut params_bytes_regular)?;

        let inner_type = Type::U32;
        let option_type = Type::option(inner_type);
        let option_value = Value::make_option(&option_type, None)?;
        let wrpc_value = WasmTypedValue(option_value, option_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for option::none"
        );
        Ok(())
    }

    #[test]
    fn test_pack_option_some() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(Some(42u32), &mut params_bytes_regular)?;

        let inner_type = Type::U32;
        let option_type = Type::option(inner_type.clone());
        let inner_value = Value::make_u32(42);
        let option_value = Value::make_option(&option_type, Some(inner_value))?;
        let wrpc_value = WasmTypedValue(option_value, option_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for option::some"
        );
        Ok(())
    }

    #[test]
    fn test_pack_result_ok() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(Result::<u32, String>::Ok(42), &mut params_bytes_regular)?;

        let ok_type = Some(Type::U32);
        let err_type = Some(Type::STRING);
        let result_type = Type::result(ok_type.clone(), err_type);
        let ok_value = Value::make_u32(42);
        let result_value = Value::make_result(&result_type, Ok(Some(ok_value)))?;
        let wrpc_value = WasmTypedValue(result_value, result_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for result::ok"
        );
        Ok(())
    }

    #[test]
    fn test_pack_result_err() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::new();
        pack(
            Result::<u32, String>::Err("error".to_string()),
            &mut params_bytes_regular,
        )?;

        let ok_type = Some(Type::U32);
        let err_type = Some(Type::STRING);
        let result_type = Type::result(ok_type, err_type.clone());
        use std::borrow::Cow;
        let err_value = Value::make_string(Cow::Borrowed("error"));
        let result_value = Value::make_result(&result_type, Err(Some(err_value)))?;
        let wrpc_value = WasmTypedValue(result_value, result_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        assert_eq!(
            params_bytes_regular.as_ref(),
            params_bytes_wave.as_ref(),
            "Regular pack and wasm-wave pack should produce identical bytes for result::err"
        );
        Ok(())
    }

    #[test]
    fn test_pack_flags() -> anyhow::Result<()> {
        // Flags encode as bit flags
        // We'll create a flags type with 3 flags: "read", "write", "execute"
        // Test encoding with "read" and "write" set
        let flags_type =
            Type::flags(["read", "write", "execute"]).expect("flags type should be valid");

        // For comparison, we can't directly compare with a Rust type easily
        // So we'll test that it encodes correctly
        let flags_value = Value::make_flags(&flags_type, ["read", "write"])?;
        let wrpc_value = WasmTypedValue(flags_value, flags_type);
        let mut params_bytes_wave = BytesMut::new();
        pack(wrpc_value, &mut params_bytes_wave)?;

        // For 3 flags, it should use u8 encoding (1 byte)
        assert_eq!(
            params_bytes_wave.len(),
            1,
            "Small flags should encode as 1 byte"
        );
        // read=bit0, write=bit1, execute=bit2
        // read + write = 0b00000011 = 3
        assert_eq!(
            params_bytes_wave[0], 0b00000011,
            "Flags with read and write should encode as 0b00000011"
        );
        Ok(())
    }
}
