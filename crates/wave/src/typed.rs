//! [`wrpc_transport::Encode`] support

use bytes::BytesMut;
use tokio_util::codec::Encoder;
use wasm_wave::value::Value;
use wasm_wave::wasm::WasmValue;
use wrpc_transport::{Deferred, DeferredFn, Encode, Sink};

use crate::ValueEncoder;

/// Value paired with its type, which implements [`Encode`]
///
/// ```
/// use bytes::BytesMut;
/// use wasm_wave::value::{Type, Value};
/// use wasm_wave::wasm::WasmValue as _;
/// use wrpc_wave::TypedValue;
///
/// let mut buf = BytesMut::default();
/// futures::executor::block_on(wrpc_transport::pack(
///     TypedValue(Value::make_u32(42), Type::U32),
///     &mut buf,
/// ))
/// .unwrap();
/// assert_eq!(buf.as_ref(), [42]);
/// ```
#[derive(Clone, Debug)]
pub struct TypedValue<V: WasmValue = Value>(pub V, pub V::Type);

/// [Encoder] of [`TypedValue`]
#[derive(Debug, Default)]
pub struct TypedValueEncoder;

impl<W> Deferred<W> for TypedValueEncoder {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        None
    }
}

impl<V: WasmValue> Encoder<&TypedValue<V>> for TypedValueEncoder {
    type Error = anyhow::Error;

    fn encode(&mut self, v: &TypedValue<V>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        ValueEncoder::new(&v.1).encode(&v.0, dst)
    }
}

impl<V: WasmValue> Encoder<TypedValue<V>> for TypedValueEncoder {
    type Error = anyhow::Error;

    fn encode(&mut self, v: TypedValue<V>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(&v, dst)
    }
}

impl<V: WasmValue, W: Sink> Encode<W> for TypedValue<V> {
    type Encoder = TypedValueEncoder;
}

impl<V: WasmValue, W: Sink> Encode<W> for &TypedValue<V> {
    type Encoder = TypedValueEncoder;
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_wave::value::Type;
    use wrpc_transport::Packer;

    fn pack<T: Encode<Packer>>(
        v: T,
        dst: &mut BytesMut,
    ) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
        futures::executor::block_on(wrpc_transport::pack(v, dst))
    }

    #[test]
    fn test_pack_bool() -> anyhow::Result<()> {
        let mut params_bytes_regular = BytesMut::default();
        pack(true, &mut params_bytes_regular)?;

        let value = Value::make_bool(true);
        let ty = Type::BOOL;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(42i8, &mut params_bytes_regular)?;

        let value = Value::make_s8(42);
        let ty = Type::S8;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(42u8, &mut params_bytes_regular)?;

        let value = Value::make_u8(42);
        let ty = Type::U8;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(1234i16, &mut params_bytes_regular)?;

        let value = Value::make_s16(1234);
        let ty = Type::S16;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(1234u16, &mut params_bytes_regular)?;

        let value = Value::make_u16(1234);
        let ty = Type::U16;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(123456i32, &mut params_bytes_regular)?;

        let value = Value::make_s32(123456);
        let ty = Type::S32;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(123456u32, &mut params_bytes_regular)?;

        let value = Value::make_u32(123456);
        let ty = Type::U32;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(1234567890i64, &mut params_bytes_regular)?;

        let value = Value::make_s64(1234567890);
        let ty = Type::S64;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(1234567890u64, &mut params_bytes_regular)?;

        let value = Value::make_u64(1234567890);
        let ty = Type::U64;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(1.5f32, &mut params_bytes_regular)?;

        let value = Value::make_f32(1.5);
        let ty = Type::F32;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(2.5f64, &mut params_bytes_regular)?;

        let value = Value::make_f64(2.5);
        let ty = Type::F64;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack('A', &mut params_bytes_regular)?;

        let value = Value::make_char('A');
        let ty = Type::CHAR;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack("hello", &mut params_bytes_regular)?;

        use std::borrow::Cow;
        let value = Value::make_string(Cow::Borrowed("hello"));
        let ty = Type::STRING;
        let wrpc_value = TypedValue(value, ty);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(vec![1u32, 2u32, 3u32], &mut params_bytes_regular)?;

        let element_type = Type::U32;
        let list_type = Type::list(element_type.clone());
        let values = vec![Value::make_u32(1), Value::make_u32(2), Value::make_u32(3)];
        let list_value = Value::make_list(&list_type, values)?;
        let wrpc_value = TypedValue(list_value, list_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack((42u32, 100u32), &mut params_bytes_regular)?;

        let record_type = Type::record([("a", Type::U32), ("b", Type::U32)])
            .expect("record type should be valid");
        let record_value = Value::make_record(
            &record_type,
            [("a", Value::make_u32(42)), ("b", Value::make_u32(100))],
        )?;
        let wrpc_value = TypedValue(record_value, record_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack((5u32,), &mut params_bytes_regular)?;

        // Test 2: Pack a wasm-wave Value representing (5,) as a tuple
        let value_5 = Value::make_u32(5);
        let tuple_type = Type::tuple([Type::U32]).expect("tuple type should be valid");
        let tuple_value = Value::make_tuple(&tuple_type, [value_5])?;
        let wrpc_tuple_value = TypedValue(tuple_value, tuple_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(Some(42u32), &mut params_bytes_regular)?;

        let variant_value = Value::make_variant(&variant_type, "some", Some(Value::make_u32(42)))?;
        let wrpc_value = TypedValue(variant_value, variant_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let wrpc_value = TypedValue(enum_value, enum_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(Option::<u32>::None, &mut params_bytes_regular)?;

        let inner_type = Type::U32;
        let option_type = Type::option(inner_type);
        let option_value = Value::make_option(&option_type, None)?;
        let wrpc_value = TypedValue(option_value, option_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(Some(42u32), &mut params_bytes_regular)?;

        let inner_type = Type::U32;
        let option_type = Type::option(inner_type.clone());
        let inner_value = Value::make_u32(42);
        let option_value = Value::make_option(&option_type, Some(inner_value))?;
        let wrpc_value = TypedValue(option_value, option_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
        pack(Result::<u32, String>::Ok(42), &mut params_bytes_regular)?;

        let ok_type = Some(Type::U32);
        let err_type = Some(Type::STRING);
        let result_type = Type::result(ok_type.clone(), err_type);
        let ok_value = Value::make_u32(42);
        let result_value = Value::make_result(&result_type, Ok(Some(ok_value)))?;
        let wrpc_value = TypedValue(result_value, result_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let mut params_bytes_regular = BytesMut::default();
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
        let wrpc_value = TypedValue(result_value, result_type);
        let mut params_bytes_wave = BytesMut::default();
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
        let wrpc_value = TypedValue(flags_value, flags_type);
        let mut params_bytes_wave = BytesMut::default();
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
