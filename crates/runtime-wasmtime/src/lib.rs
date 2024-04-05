use core::iter::zip;

use anyhow::{bail, ensure, Context as _};
use futures::stream;
use tracing::instrument;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{
    Enum, Flags, List, OptionVal, Record, ResourceType, ResultVal, Tuple, Type, Val, Variant,
};
use wasmtime::AsContextMut;
use wasmtime_wasi::{InputStream, Pollable, WasiView};

#[instrument(level = "trace", skip(store))]
pub fn to_wrpc_value<T: WasiView>(
    mut store: impl AsContextMut<Data = T>,
    val: &Val,
) -> anyhow::Result<wrpc_transport::Value> {
    let mut store = store.as_context_mut();
    match val {
        Val::Bool(val) => Ok(wrpc_transport::Value::Bool(*val)),
        Val::S8(val) => Ok(wrpc_transport::Value::S8(*val)),
        Val::U8(val) => Ok(wrpc_transport::Value::U8(*val)),
        Val::S16(val) => Ok(wrpc_transport::Value::S16(*val)),
        Val::U16(val) => Ok(wrpc_transport::Value::U16(*val)),
        Val::S32(val) => Ok(wrpc_transport::Value::S32(*val)),
        Val::U32(val) => Ok(wrpc_transport::Value::U32(*val)),
        Val::S64(val) => Ok(wrpc_transport::Value::S64(*val)),
        Val::U64(val) => Ok(wrpc_transport::Value::U64(*val)),
        Val::Float32(val) => Ok(wrpc_transport::Value::F32(*val)),
        Val::Float64(val) => Ok(wrpc_transport::Value::F64(*val)),
        Val::Char(val) => Ok(wrpc_transport::Value::Char(*val)),
        Val::String(val) => Ok(wrpc_transport::Value::String(val.to_string())),
        Val::List(val) => val
            .iter()
            .map(|val| to_wrpc_value(&mut store, val))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport::Value::List),
        Val::Record(val) => val
            .fields()
            .map(|(_, val)| val)
            .map(|val| to_wrpc_value(&mut store, val))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport::Value::Record),
        Val::Tuple(val) => val
            .values()
            .iter()
            .map(|val| to_wrpc_value(&mut store, val))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport::Value::Tuple),
        Val::Variant(val) => {
            let discriminant = zip(0.., val.ty().cases())
                .find_map(|(i, Case { name, .. })| (name == val.discriminant()).then_some(i))
                .context("unknown variant discriminant")?;
            let nested = val
                .payload()
                .map(|val| to_wrpc_value(store, val))
                .transpose()?
                .map(Box::new);
            Ok(wrpc_transport::Value::Variant {
                discriminant,
                nested,
            })
        }
        Val::Enum(val) => zip(0.., val.ty().names())
            .find_map(|(i, name)| (name == val.discriminant()).then_some(i))
            .context("unknown enum discriminant")
            .map(wrpc_transport::Value::Enum),
        Val::Option(val) => {
            let val = val
                .value()
                .map(|val| to_wrpc_value(store, val))
                .transpose()?
                .map(Box::new);
            Ok(wrpc_transport::Value::Option(val))
        }
        Val::Result(val) => {
            let val = match val.value() {
                Ok(val) => {
                    let val = val
                        .map(|val| to_wrpc_value(store, val))
                        .transpose()?
                        .map(Box::new);
                    Ok(val)
                }
                Err(val) => {
                    let val = val
                        .map(|val| to_wrpc_value(store, val))
                        .transpose()?
                        .map(Box::new);
                    Err(val)
                }
            };
            Ok(wrpc_transport::Value::Result(val))
        }
        Val::Flags(val) => {
            let mut v = 0;
            for name in val.flags() {
                let i = zip(0.., val.ty().names())
                    .find_map(|(i, flag_name)| (name == flag_name).then_some(i))
                    .context("unknown flag")?;
                ensure!(
                    i < 64,
                    "flag discriminants over 64 currently cannot be represented"
                );
                v |= 1 << i
            }
            Ok(wrpc_transport::Value::Flags(v))
        }
        Val::Resource(resource) => {
            let ty = resource.ty();
            if ty == ResourceType::host::<InputStream>() {
                let _stream = resource
                    .try_into_resource::<InputStream>(&mut store)
                    .context("failed to downcast `wasi:io/input-stream`")?;
                //if stream.owned() {
                //    store
                //        .data_mut()
                //        .table()
                //        .delete(stream)
                //        .context("failed to delete input stream")?;
                //} else {
                //    store
                //        .data_mut()
                //        .table()
                //        .get_mut(&stream)
                //        .context("failed to get input stream")?;
                //};
                Ok(wrpc_transport::Value::Stream(Box::pin(stream::once(
                    async { bail!("`wasi:io/input-stream` not supported yet") },
                ))))
            } else if ty == ResourceType::host::<Pollable>() {
                let _pollable = resource
                    .try_into_resource::<Pollable>(&mut store)
                    .context("failed to downcast `wasi:io/pollable")?;
                //if pollable.owned() {
                //    store
                //        .data_mut()
                //        .table()
                //        .delete(pollable)
                //        .context("failed to delete pollable")?;
                //} else {
                //    store
                //        .data_mut()
                //        .table()
                //        .get_mut(&pollable)
                //        .context("failed to get pollable")?;
                //}
                Ok(wrpc_transport::Value::Future(Box::pin(async {
                    bail!("`wasi:io/pollable` not supported yet")
                })))
            } else {
                bail!("resources not supported yet")
            }
        }
    }
}

#[instrument(level = "trace", skip(val))]
pub fn from_wrpc_value(val: wrpc_transport::Value, ty: &Type) -> anyhow::Result<Val> {
    match (val, ty) {
        (wrpc_transport::Value::Bool(v), Type::Bool) => Ok(Val::Bool(v)),
        (wrpc_transport::Value::U8(v), Type::U8) => Ok(Val::U8(v)),
        (wrpc_transport::Value::U16(v), Type::U16) => Ok(Val::U16(v)),
        (wrpc_transport::Value::U32(v), Type::U32) => Ok(Val::U32(v)),
        (wrpc_transport::Value::U64(v), Type::U64) => Ok(Val::U64(v)),
        (wrpc_transport::Value::S8(v), Type::S8) => Ok(Val::S8(v)),
        (wrpc_transport::Value::S16(v), Type::S16) => Ok(Val::S16(v)),
        (wrpc_transport::Value::S32(v), Type::S32) => Ok(Val::S32(v)),
        (wrpc_transport::Value::S64(v), Type::S64) => Ok(Val::S64(v)),
        (wrpc_transport::Value::F32(v), Type::Float32) => Ok(Val::Float32(v)),
        (wrpc_transport::Value::F64(v), Type::Float64) => Ok(Val::Float64(v)),
        (wrpc_transport::Value::Char(v), Type::Char) => Ok(Val::Char(v)),
        (wrpc_transport::Value::String(v), Type::String) => Ok(Val::String(v.into())),
        (wrpc_transport::Value::List(vs), Type::List(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            let el_ty = ty.ty();
            for v in vs {
                let v = from_wrpc_value(v, &el_ty).context("failed to convert list element")?;
                w_vs.push(v);
            }
            List::new(ty, w_vs.into()).map(Val::List)
        }
        (wrpc_transport::Value::Record(vs), Type::Record(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, Field { name, ty }) in zip(vs, ty.fields()) {
                let v = from_wrpc_value(v, &ty).context("failed to convert record field")?;
                w_vs.push((name, v));
            }
            Record::new(ty, w_vs).map(Val::Record)
        }
        (wrpc_transport::Value::Tuple(vs), Type::Tuple(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, ty) in zip(vs, ty.types()) {
                let v = from_wrpc_value(v, &ty).context("failed to convert tuple element")?;
                w_vs.push(v);
            }
            Tuple::new(ty, w_vs.into()).map(Val::Tuple)
        }
        (
            wrpc_transport::Value::Variant {
                discriminant,
                nested,
            },
            Type::Variant(ty),
        ) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            let Case { name, ty: case_ty } = ty
                .cases()
                .nth(discriminant)
                .context("variant discriminant not found")?;
            let v = if let Some(case_ty) = case_ty {
                let v = nested.context("nested value missing")?;
                let v = from_wrpc_value(*v, &case_ty).context("failed to convert variant value")?;
                Some(v)
            } else {
                None
            };
            Variant::new(ty, name, v).map(Val::Variant)
        }
        (wrpc_transport::Value::Enum(discriminant), Type::Enum(ty)) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            let name = ty
                .names()
                .nth(discriminant)
                .context("enum discriminant not found")?;
            Enum::new(ty, name).map(Val::Enum)
        }
        (wrpc_transport::Value::Option(v), Type::Option(ty)) => {
            let v = if let Some(v) = v {
                let v = from_wrpc_value(*v, &ty.ty()).context("failed to convert option value")?;
                Some(v)
            } else {
                None
            };
            OptionVal::new(ty, v).map(Val::Option)
        }
        (wrpc_transport::Value::Result(v), Type::Result(ty)) => {
            let v = match v {
                Ok(None) => Ok(None),
                Ok(Some(v)) => {
                    let ty = ty.ok().context("`result::ok` type missing")?;
                    let v =
                        from_wrpc_value(*v, &ty).context("failed to convert `result::ok` value")?;
                    Ok(Some(v))
                }
                Err(None) => Err(None),
                Err(Some(v)) => {
                    let ty = ty.err().context("`result::err` type missing")?;
                    let v = from_wrpc_value(*v, &ty)
                        .context("failed to convert `result::err` value")?;
                    Err(Some(v))
                }
            };
            ResultVal::new(ty, v).map(Val::Result)
        }
        (wrpc_transport::Value::Flags(v), Type::Flags(ty)) => {
            // NOTE: Currently flags are limited to 64
            let mut names = Vec::with_capacity(64);
            for (i, name) in zip(0..64, ty.names()) {
                if v & (1 << i) != 0 {
                    names.push(name)
                }
            }
            Flags::new(ty, &names).map(Val::Flags)
        }
        (wrpc_transport::Value::Future(_v), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<Pollable>() {
                // TODO: Implement once https://github.com/bytecodealliance/wasmtime/issues/7714
                // is addressed
                bail!("`wasi:io/pollable` not supported yet")
            } else {
                // TODO: Implement in preview3 or via a wasmCloud-specific interface
                bail!("dynamically-typed futures not supported yet")
            }
        }
        (wrpc_transport::Value::Stream(_v), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<InputStream>() {
                // TODO: Implement once https://github.com/bytecodealliance/wasmtime/issues/7714
                // is addressed
                bail!("`wasi:io/input-stream` not supported yet")
            } else {
                // TODO: Implement in preview3 or via a wasmCloud-specific interface
                bail!("dynamically-typed streams not supported yet")
            }
        }
        (wrpc_transport::Value::String(_), Type::Own(_ty) | Type::Borrow(_ty)) => {
            // TODO: Implement guest resource handling
            bail!("resources not supported yet")
        }
        _ => bail!("type mismatch"),
    }
}
