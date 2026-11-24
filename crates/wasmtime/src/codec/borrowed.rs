use core::borrow::Borrow;
use core::iter::{self, zip};

use std::collections::HashSet;

use bytes::{BufMut as _, BytesMut};
use tokio_util::codec::Encoder as _;
use wasm_tokio::cm::{F32Codec, F64Codec};
use wasm_tokio::{CoreNameEncoder, Leb128Encoder, Utf8Codec};
use wasmtime::bail;
use wasmtime::component::types::Field;
use wasmtime::component::{Type, Val};
use wasmtime::error::Context as _;

use crate::codec::{check_fields, encode_enum_case, encode_flags, encode_variant_case};

pub(super) fn encode_seq<'a>(
    tys: impl IntoIterator<Item: Borrow<Type>>,
    vs: impl IntoIterator<Item = &'a Val>,
    dst: &mut BytesMut,
) -> wasmtime::Result<()> {
    for (i, (v, ty)) in zip(vs, tys).enumerate() {
        encode_value(ty.borrow(), v, dst)
            .with_context(|| format!("failed to encode element {i}"))?;
    }
    Ok(())
}

fn encode_payload(ty: Option<Type>, v: Option<&Val>, dst: &mut BytesMut) -> wasmtime::Result<()> {
    match (v, ty) {
        (Some(v), Some(ty)) => encode_value(&ty, v, dst).context("failed to encode payload"),
        (Some(..), None) => bail!("payload of unknown type"),
        (None, Some(..)) => bail!("payload missing"),
        (None, None) => Ok(()),
    }
}

pub(super) fn encode_value(ty: &Type, v: &Val, dst: &mut BytesMut) -> wasmtime::Result<()> {
    match (v, ty) {
        (Val::Bool(v), Type::Bool) => dst.put_u8((*v).into()),
        (Val::S8(v), Type::S8) => dst.put_i8(*v),
        (Val::U8(v), Type::U8) => dst.put_u8(*v),
        (Val::S16(v), Type::S16) => Leb128Encoder.encode(*v, dst)?,
        (Val::U16(v), Type::U16) => Leb128Encoder.encode(*v, dst)?,
        (Val::S32(v), Type::S32) => Leb128Encoder.encode(*v, dst)?,
        (Val::U32(v), Type::U32) => Leb128Encoder.encode(*v, dst)?,
        (Val::S64(v), Type::S64) => Leb128Encoder.encode(*v, dst)?,
        (Val::U64(v), Type::U64) => Leb128Encoder.encode(*v, dst)?,
        (Val::Float32(v), Type::Float32) => F32Codec.encode(*v, dst)?,
        (Val::Float64(v), Type::Float64) => F64Codec.encode(*v, dst)?,
        (Val::Char(v), Type::Char) => Utf8Codec.encode(*v, dst)?,
        (Val::String(v), Type::String) => CoreNameEncoder.encode(v.as_str(), dst)?,
        (Val::List(vs), Type::List(ty)) => {
            let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
            Leb128Encoder.encode(n, dst)?;
            encode_seq(iter::repeat(ty.ty()), vs, dst)?;
        }
        (Val::Map(vs), Type::Map(ty)) => {
            let n = u32::try_from(vs.len()).context("map length does not fit in u32")?;
            Leb128Encoder.encode(n, dst)?;
            let (key, value) = (ty.key(), ty.value());
            let mut keys = HashSet::<Vec<u8>>::default();
            for (i, (k, v)) in vs.iter().enumerate() {
                let off = dst.len();
                encode_value(&key, k, dst).with_context(|| format!("failed to encode key {i}"))?;
                if !keys.insert(dst[off..].to_vec()) {
                    bail!("duplicate key {i}");
                }
                encode_value(&value, v, dst)
                    .with_context(|| format!("failed to encode value {i}"))?;
            }
        }
        (Val::FixedLengthList(vs), Type::FixedLengthList(ty)) => {
            if u32::try_from(vs.len()) != Ok(ty.len()) {
                bail!("list has {} elements, expected {}", vs.len(), ty.len());
            }
            encode_seq(iter::repeat(ty.ty()), vs, dst)?;
        }
        (Val::Record(vs), Type::Record(ty)) => {
            check_fields(ty, vs.iter().map(|(name, ..)| name.as_str()))?;
            let tys = ty.fields().map(|Field { ty, .. }| ty);
            encode_seq(tys, vs.iter().map(|(.., v)| v), dst)?;
        }
        (Val::Tuple(vs), Type::Tuple(ty)) => {
            if vs.len() != ty.types().len() {
                bail!(
                    "tuple has {} elements, expected {}",
                    vs.len(),
                    ty.types().len()
                );
            }
            encode_seq(ty.types(), vs, dst)?;
        }
        (Val::Variant(case, v), Type::Variant(ty)) => {
            let ty = encode_variant_case(ty, case, dst)?;
            encode_payload(ty, v.as_deref(), dst)?;
        }
        (Val::Enum(case), Type::Enum(ty)) => encode_enum_case(ty, case, dst)?,
        (Val::Option(None), Type::Option(..)) => dst.put_u8(0),
        (Val::Option(Some(v)), Type::Option(ty)) => {
            dst.put_u8(1);
            encode_value(&ty.ty(), v, dst).context("failed to encode `option::some` value")?;
        }
        (Val::Result(Ok(v)), Type::Result(ty)) => {
            dst.put_u8(0);
            encode_payload(ty.ok(), v.as_deref(), dst)?;
        }
        (Val::Result(Err(v)), Type::Result(ty)) => {
            dst.put_u8(1);
            encode_payload(ty.err(), v.as_deref(), dst)?;
        }
        (Val::Flags(vs), Type::Flags(ty)) => encode_flags(ty, vs.iter().map(String::as_str), dst)?,
        (Val::Resource(..), ..) => bail!("resources require a store"),
        (Val::Future(..), ..) => bail!("`future`s require a store"),
        (Val::Stream(..), ..) => bail!("`stream`s require a store"),
        (Val::ErrorContext(..), ..) => bail!("`error-context` not supported"),
        _ => bail!("value type mismatch"),
    }
    Ok(())
}
