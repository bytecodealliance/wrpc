//! Value encoder for wasm-wave traits that works similarly to `ValEncoder`

use core::iter::zip;
use core::ops::{BitOrAssign, Shl};

use std::collections::HashSet;

use anyhow::{bail, Context as _};
use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder;
use tracing::instrument;
use wasm_tokio::{CoreNameEncoder, Leb128Encoder, Utf8Codec};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue};

/// Encoder for wasm-wave values with type information stored in the encoder.
/// This mirrors the `ValEncoder` from `wrpc-runtime-wasmtime`.
///
/// ## Usage
///
/// ```no_run
/// use wrpc_wave::WaveEncoder;
/// use wasm_wave::value::{Value, Type};
/// use wasm_wave::wasm::WasmValue;
/// use bytes::BytesMut;
/// use wit_bindgen_wrpc::tokio_util::codec::Encoder;
///
/// let value = Value::make_u32(42);
/// let ty = Type::U32;
/// let mut encoder = WaveEncoder::new(&ty);
/// let mut buf = BytesMut::new();
/// encoder.encode(&value, &mut buf).unwrap();
/// ```
///
/// Note:
/// this crate is runtime-agnostic, which means it has no access to a "store"
/// This means it cannot handle resources or any async encoding
#[derive(Debug)]
pub struct WaveEncoder<'a, T: WasmType> {
    /// The type information for encoding
    pub ty: &'a T,
}

impl<'a, T: WasmType> WaveEncoder<'a, T> {
    /// Creates a new `WaveEncoder` with the given type information.
    #[must_use]
    pub fn new(ty: &'a T) -> Self {
        Self { ty }
    }

    /// Creates a new encoder with a different type, reusing the current encoder's context.
    /// This is useful for encoding nested values with different types.
    #[must_use]
    pub fn with_type<'b>(&'b mut self, ty: &'b T) -> WaveEncoder<'b, T> {
        WaveEncoder { ty }
    }
}
fn find_enum_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    names: impl IntoIterator<Item = &'a str>,
    discriminant: &str,
) -> anyhow::Result<T> {
    zip(iter, names)
        .find_map(|(i, name)| (name == discriminant).then_some(i))
        .context("unknown enum discriminant")
}

fn find_variant_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    cases: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    discriminant: &str,
) -> anyhow::Result<(T, Option<&'a str>)> {
    zip(iter, cases)
        .find_map(|(i, (name, ty))| (name == discriminant).then_some((i, ty)))
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

// Generic implementation for any type implementing WasmValue and WasmType
impl<'a, V, T> Encoder<&V> for WaveEncoder<'a, T>
where
    V: WasmValue<Type = T>,
    T: WasmType,
{
    type Error = anyhow::Error;

    #[allow(clippy::too_many_lines)]
    #[instrument(level = "trace", skip(self, val))]
    fn encode(&mut self, val: &V, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match val.kind() {
            WasmTypeKind::Bool => {
                dst.reserve(1);
                dst.put_u8(val.unwrap_bool().into());
                Ok(())
            }
            WasmTypeKind::S8 => {
                dst.reserve(1);
                dst.put_i8(val.unwrap_s8());
                Ok(())
            }
            WasmTypeKind::U8 => {
                dst.reserve(1);
                dst.put_u8(val.unwrap_u8());
                Ok(())
            }
            WasmTypeKind::S16 => Leb128Encoder
                .encode(val.unwrap_s16(), dst)
                .context("failed to encode s16"),
            WasmTypeKind::U16 => Leb128Encoder
                .encode(val.unwrap_u16(), dst)
                .context("failed to encode u16"),
            WasmTypeKind::S32 => Leb128Encoder
                .encode(val.unwrap_s32(), dst)
                .context("failed to encode s32"),
            WasmTypeKind::U32 => Leb128Encoder
                .encode(val.unwrap_u32(), dst)
                .context("failed to encode u32"),
            WasmTypeKind::S64 => Leb128Encoder
                .encode(val.unwrap_s64(), dst)
                .context("failed to encode s64"),
            WasmTypeKind::U64 => Leb128Encoder
                .encode(val.unwrap_u64(), dst)
                .context("failed to encode u64"),
            WasmTypeKind::F32 => {
                dst.reserve(4);
                dst.put_f32_le(val.unwrap_f32());
                Ok(())
            }
            WasmTypeKind::F64 => {
                dst.reserve(8);
                dst.put_f64_le(val.unwrap_f64());
                Ok(())
            }
            WasmTypeKind::Char => Utf8Codec
                .encode(val.unwrap_char(), dst)
                .context("failed to encode char"),
            WasmTypeKind::String => CoreNameEncoder
                .encode(val.unwrap_string().as_ref(), dst)
                .context("failed to encode string"),
            WasmTypeKind::List => {
                let elements: Vec<_> = val.unwrap_list().map(|v| v.into_owned()).collect();
                let n = u32::try_from(elements.len()).context("list length does not fit in u32")?;
                dst.reserve(5 + elements.len());
                Leb128Encoder
                    .encode(n, dst)
                    .context("failed to encode list length")?;
                let element_type = self
                    .ty
                    .list_element_type()
                    .context("list type should have element type")?;
                for element in elements {
                    let mut enc = self.with_type(&element_type);
                    enc.encode(&element, dst)
                        .context("failed to encode list element")?;
                }
                Ok(())
            }
            WasmTypeKind::Record => {
                let fields: Vec<_> = val
                    .unwrap_record()
                    .map(|(name, v)| (name.into_owned(), v.into_owned()))
                    .collect();
                let field_types: Vec<_> = self.ty.record_fields().map(|(_, ty)| ty).collect();
                dst.reserve(fields.len());
                for ((_name, field_value), field_type) in fields.iter().zip(field_types.iter()) {
                    let mut enc = self.with_type(field_type);
                    enc.encode(field_value, dst)
                        .context("failed to encode record field")?;
                }
                Ok(())
            }
            WasmTypeKind::Tuple => {
                let elements: Vec<_> = val.unwrap_tuple().map(|v| v.into_owned()).collect();
                let element_types: Vec<_> = self.ty.tuple_element_types().collect();
                dst.reserve(elements.len());
                for (element, element_type) in elements.iter().zip(element_types.iter()) {
                    let mut enc = self.with_type(element_type);
                    enc.encode(element, dst)
                        .context("failed to encode tuple element")?;
                }
                Ok(())
            }
            WasmTypeKind::Variant => {
                let (case_name, payload) = val.unwrap_variant();
                let case_name = case_name.into_owned();

                // Get the type to find the discriminant index
                let cases: Vec<_> = self
                    .ty
                    .variant_cases()
                    .map(|(name, payload_ty)| (name.into_owned(), payload_ty))
                    .collect();

                let (discriminant_idx, _case_ty) = find_variant_discriminant(
                    0u32..,
                    cases
                        .iter()
                        .map(|(name, _ty)| (name.as_str(), None::<&str>)),
                    case_name.as_str(),
                )?;

                match cases.len() {
                    ..=0x0000_00ff => {
                        dst.reserve(2 + usize::from(payload.is_some()));
                        Leb128Encoder.encode(discriminant_idx as u8, dst)?;
                    }
                    0x0000_0100..=0x0000_ffff => {
                        dst.reserve(3 + usize::from(payload.is_some()));
                        Leb128Encoder.encode(discriminant_idx as u16, dst)?;
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        dst.reserve(4 + usize::from(payload.is_some()));
                        Leb128Encoder.encode(discriminant_idx, dst)?;
                    }
                    0x0100_0000..=0xffff_ffff => {
                        dst.reserve(5 + usize::from(payload.is_some()));
                        Leb128Encoder.encode(discriminant_idx, dst)?;
                    }
                    _ => bail!("case count does not fit in u32"),
                }

                if let Some(payload_val) = payload {
                    // Find the payload type for this variant case
                    let payload_type = self
                        .ty
                        .variant_cases()
                        .find_map(|(name, payload_ty)| (name == case_name).then_some(payload_ty))
                        .flatten()
                        .context("variant case should have payload type")?;
                    let mut enc = self.with_type(&payload_type);
                    enc.encode(&payload_val.into_owned(), dst)
                        .context("failed to encode variant payload")?;
                }
                Ok(())
            }
            WasmTypeKind::Enum => {
                let case_name = val.unwrap_enum().into_owned();

                // Get the type to find the discriminant index
                let names: Vec<_> = self.ty.enum_cases().map(|s| s.into_owned()).collect();

                let discriminant_idx = find_enum_discriminant(
                    0u32..,
                    names.iter().map(|s| s.as_str()),
                    case_name.as_str(),
                )?;

                match names.len() {
                    ..=0x0000_00ff => {
                        dst.reserve(2);
                        Leb128Encoder.encode(discriminant_idx as u8, dst)?;
                    }
                    0x0000_0100..=0x0000_ffff => {
                        dst.reserve(3);
                        Leb128Encoder.encode(discriminant_idx as u16, dst)?;
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        dst.reserve(4);
                        Leb128Encoder.encode(discriminant_idx, dst)?;
                    }
                    0x0100_0000..=0xffff_ffff => {
                        dst.reserve(5);
                        Leb128Encoder.encode(discriminant_idx, dst)?;
                    }
                    _ => bail!("name count does not fit in u32"),
                }
                Ok(())
            }
            WasmTypeKind::Option => match val.unwrap_option() {
                None => {
                    dst.reserve(1);
                    dst.put_u8(0);
                    Ok(())
                }
                Some(inner) => {
                    dst.reserve(2);
                    dst.put_u8(1);
                    let inner_type = self
                        .ty
                        .option_some_type()
                        .context("option type should have some type")?;
                    let mut enc = self.with_type(&inner_type);
                    enc.encode(&inner.into_owned(), dst)
                        .context("failed to encode `option::some` value")?;
                    Ok(())
                }
            },
            WasmTypeKind::Result => {
                let (ok_type, err_type) = self
                    .ty
                    .result_types()
                    .context("result type should have ok and err types")?;
                match val.unwrap_result() {
                    Ok(ok_val) => {
                        match ok_val {
                            Some(val) => {
                                dst.reserve(2);
                                dst.put_u8(0);
                                if let Some(ok_ty) = ok_type {
                                    let mut enc = self.with_type(&ok_ty);
                                    enc.encode(&val.into_owned(), dst)
                                        .context("failed to encode `result::ok` value")?;
                                }
                            }
                            None => {
                                dst.reserve(1);
                                dst.put_u8(0);
                            }
                        }
                        Ok(())
                    }
                    Err(err_val) => {
                        match err_val {
                            Some(val) => {
                                dst.reserve(2);
                                dst.put_u8(1);
                                if let Some(err_ty) = err_type {
                                    let mut enc = self.with_type(&err_ty);
                                    enc.encode(&val.into_owned(), dst)
                                        .context("failed to encode `result::err` value")?;
                                }
                            }
                            None => {
                                dst.reserve(1);
                                dst.put_u8(1);
                            }
                        }
                        Ok(())
                    }
                }
            }
            WasmTypeKind::Flags => {
                let flag_names: Vec<_> = val.unwrap_flags().map(|s| s.into_owned()).collect();

                // Get the type to know all possible flag names for bit encoding
                let all_names: Vec<_> = self.ty.flags_names().map(|s| s.into_owned()).collect();

                let flags_set: HashSet<&str> = flag_names.iter().map(|s| s.as_str()).collect();
                let vs = flag_names.iter().map(String::as_str);

                match all_names.len() {
                    ..=8 => {
                        dst.reserve(1);
                        dst.put_u8(flag_bits(all_names.iter().map(|s| s.as_str()), vs));
                    }
                    9..=16 => {
                        dst.reserve(2);
                        dst.put_u16_le(flag_bits(all_names.iter().map(|s| s.as_str()), vs));
                    }
                    17..=24 => {
                        dst.reserve(3);
                        dst.put_slice(
                            &u32::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..3],
                        );
                    }
                    25..=32 => {
                        dst.reserve(4);
                        dst.put_u32_le(flag_bits(all_names.iter().map(|s| s.as_str()), vs));
                    }
                    33..=40 => {
                        dst.reserve(5);
                        dst.put_slice(
                            &u64::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..5],
                        );
                    }
                    41..=48 => {
                        dst.reserve(6);
                        dst.put_slice(
                            &u64::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..6],
                        );
                    }
                    49..=56 => {
                        dst.reserve(7);
                        dst.put_slice(
                            &u64::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..7],
                        );
                    }
                    57..=64 => {
                        dst.reserve(8);
                        dst.put_u64_le(flag_bits(all_names.iter().map(|s| s.as_str()), vs));
                    }
                    65..=72 => {
                        dst.reserve(9);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..9],
                        );
                    }
                    73..=80 => {
                        dst.reserve(10);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..10],
                        );
                    }
                    81..=88 => {
                        dst.reserve(11);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..11],
                        );
                    }
                    89..=96 => {
                        dst.reserve(12);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..12],
                        );
                    }
                    97..=104 => {
                        dst.reserve(13);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..13],
                        );
                    }
                    105..=112 => {
                        dst.reserve(14);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..14],
                        );
                    }
                    113..=120 => {
                        dst.reserve(15);
                        dst.put_slice(
                            &u128::to_le_bytes(flag_bits(all_names.iter().map(|s| s.as_str()), vs))
                                [..15],
                        );
                    }
                    121..=128 => {
                        dst.reserve(16);
                        dst.put_u128_le(flag_bits(all_names.iter().map(|s| s.as_str()), vs));
                    }
                    bits @ 129.. => {
                        let mut cap = bits / 8;
                        if bits % 8 != 0 {
                            cap = cap.saturating_add(1);
                        }
                        let mut buf = vec![0; cap];
                        for (i, name) in all_names.iter().enumerate() {
                            if flags_set.contains(name.as_str()) {
                                buf[i / 8] |= 1 << (i % 8);
                            }
                        }
                        dst.extend_from_slice(&buf);
                    }
                }
                Ok(())
            }
            WasmTypeKind::Unsupported => {
                bail!("unsupported value type")
            }
            _ => bail!("unsupported value type: {:?}", val.kind()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use wasm_wave::value::{Type, Value};

    #[test]
    fn test_encode_bool_true() -> anyhow::Result<()> {
        let value = Value::make_bool(true);
        let ty = Type::BOOL;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[1u8]);
        Ok(())
    }

    #[test]
    fn test_encode_bool_false() -> anyhow::Result<()> {
        let value = Value::make_bool(false);
        let ty = Type::BOOL;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[0u8]);
        Ok(())
    }

    // ====== Integer Types Tests ======

    #[test]
    fn test_encode_s8_positive() -> anyhow::Result<()> {
        let value = Value::make_s8(42);
        let ty = Type::S8;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[42u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s8_negative() -> anyhow::Result<()> {
        let value = Value::make_s8(-42);
        let ty = Type::S8;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[(-42i8) as u8]);
        Ok(())
    }

    #[test]
    fn test_encode_u8() -> anyhow::Result<()> {
        let value = Value::make_u8(255);
        let ty = Type::U8;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &[255u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s16() -> anyhow::Result<()> {
        let value = Value::make_s16(1000);
        let ty = Type::S16;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // 1000 in LEB128: 0xE8 0x07
        assert_eq!(buf.as_ref(), &[0xE8, 0x07]);
        Ok(())
    }

    #[test]
    fn test_encode_u16() -> anyhow::Result<()> {
        let value = Value::make_u16(1000);
        let ty = Type::U16;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // 1000 in LEB128: 0xE8 0x07
        assert_eq!(buf.as_ref(), &[0xE8, 0x07]);
        Ok(())
    }

    #[test]
    fn test_encode_s32() -> anyhow::Result<()> {
        let value = Value::make_s32(100000);
        let ty = Type::S32;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // 100000 in LEB128: 0xA0 0x8D 0x06
        assert_eq!(buf.as_ref(), &[0xA0, 0x8D, 0x06]);
        Ok(())
    }

    #[test]
    fn test_encode_u32() -> anyhow::Result<()> {
        let value = Value::make_u32(42);
        let ty = Type::U32;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // 42 in LEB128 is just [42]
        assert_eq!(buf.as_ref(), &[42u8]);
        Ok(())
    }

    #[test]
    fn test_encode_s64() -> anyhow::Result<()> {
        let value = Value::make_s64(-1);
        let ty = Type::S64;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // -1 in LEB128: 0x7F
        assert_eq!(buf.as_ref(), &[0x7F]);
        Ok(())
    }

    #[test]
    fn test_encode_u64() -> anyhow::Result<()> {
        let value = Value::make_u64(12345);
        let ty = Type::U64;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // 12345 in LEB128: 0xB9 0x60
        assert_eq!(buf.as_ref(), &[0xB9, 0x60]);
        Ok(())
    }

    // ====== Float Types Tests ======

    #[test]
    fn test_encode_f32() -> anyhow::Result<()> {
        let value = Value::make_f32(3.14);
        let ty = Type::F32;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &3.14f32.to_le_bytes());
        Ok(())
    }

    #[test]
    fn test_encode_f64() -> anyhow::Result<()> {
        let value = Value::make_f64(3.14159265359);
        let ty = Type::F64;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        assert_eq!(buf.as_ref(), &3.14159265359f64.to_le_bytes());
        Ok(())
    }

    // ====== Char Tests ======

    #[test]
    fn test_encode_char() -> anyhow::Result<()> {
        let value = Value::make_char('A');
        let ty = Type::CHAR;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
        encoder.encode(&value, &mut buf)?;

        // UTF-8 encoding of 'A' (0x41)
        assert_eq!(buf.as_ref(), &[0x41]);
        Ok(())
    }

    #[test]
    fn test_encode_char_unicode() -> anyhow::Result<()> {
        let value = Value::make_char('😀');
        let ty = Type::CHAR;
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
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
        let mut encoder = WaveEncoder::new(&ty);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&list_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&list_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&record_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&tuple_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&variant_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&variant_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&enum_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&option_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&option_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&result_type);
        let mut buf = BytesMut::new();
        encoder.encode(&result_value, &mut buf)?;

        // Ok(42): discriminant 0, value 42
        assert_eq!(buf.as_ref(), &[0, 42]);
        Ok(())
    }

    #[test]
    fn test_encode_result_ok_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(None, Some(Type::STRING));
        let result_value = Value::make_result(&result_type, Ok(None))?;

        let mut encoder = WaveEncoder::new(&result_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&result_type);
        let mut buf = BytesMut::new();
        encoder.encode(&result_value, &mut buf)?;

        // Err("fail"): discriminant 1, length 4, "fail"
        assert_eq!(buf.as_ref(), &[1, 4, b'f', b'a', b'i', b'l']);
        Ok(())
    }

    #[test]
    fn test_encode_result_err_no_value() -> anyhow::Result<()> {
        let result_type = Type::result(Some(Type::U32), None);
        let result_value = Value::make_result(&result_type, Err(None))?;

        let mut encoder = WaveEncoder::new(&result_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&flags_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&flags_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&flags_type);
        let mut buf = BytesMut::new();
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

        let mut encoder = WaveEncoder::new(&flags_type);
        let mut buf = BytesMut::new();
        encoder.encode(&flags_value, &mut buf)?;

        // f0, f8, f16 set: bit 0 in byte 0, bit 0 in byte 1, bit 0 in byte 2
        assert_eq!(buf.as_ref(), &[0x01, 0x01, 0x01]);
        Ok(())
    }
}
