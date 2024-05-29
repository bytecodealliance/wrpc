#![allow(clippy::type_complexity)] // TODO: https://github.com/wrpc/wrpc/issues/2

use core::fmt::{self, Display};
use core::future::Future;
use core::iter::zip;
use core::mem;
use core::ops::{BitOr, BitOrAssign, Shl};
use core::pin::{pin, Pin};
use core::task::{Context, Poll};

use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use bytes::{BufMut as _, Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::try_join;
use tokio_util::codec::Encoder;
use tracing::{error, trace};
use tracing::{instrument, warn};
use wasm_tokio::cm::{AsyncReadValue as _, CharEncoder};
use wasm_tokio::{AsyncReadCore as _, CoreStringEncoder, Leb128Encoder};
use wasmtime::component::types::{self, Case, Field};
use wasmtime::component::{Linker, ResourceType, Type, Val};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::{
    FileInputStream, HostInputStream, InputStream, Pollable, StreamError, StreamResult, Subscribe,
    WasiView,
};
use wit_parser::FunctionKind;
use wrpc_introspect::rpc_func_name;
use wrpc_transport_next::{Invocation, Invoke, Session};

pub struct RemoteResource(pub String);

pub struct OutgoingHostInputStream(Box<dyn HostInputStream>);

#[derive(Debug)]
pub enum OutgoingStreamError {
    Failed(anyhow::Error),
    Trap(anyhow::Error),
}

impl Display for OutgoingStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(err) => write!(f, "last operation failed: {err:#}"),
            Self::Trap(err) => write!(f, "trap: {err:#}"),
        }
    }
}

impl Error for OutgoingStreamError {}

impl Stream for OutgoingHostInputStream {
    type Item = Result<Bytes, OutgoingStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match pin!(self.0.ready()).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(()) => {}
        }
        match self.0.read(8096) {
            Ok(buf) => Poll::Ready(Some(Ok(buf))),
            Err(StreamError::LastOperationFailed(err)) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Failed(err))))
            }
            Err(StreamError::Trap(err)) => Poll::Ready(Some(Err(OutgoingStreamError::Trap(err)))),
            Err(StreamError::Closed) => Poll::Ready(None),
        }
    }
}

pub struct OutgoingFileInputStream(FileInputStream);

impl Stream for OutgoingFileInputStream {
    type Item = Result<Bytes, OutgoingStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match pin!(self.0.read(8096)).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(buf)) => Poll::Ready(Some(Ok(buf))),
            Poll::Ready(Err(StreamError::LastOperationFailed(err))) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Failed(err))))
            }
            Poll::Ready(Err(StreamError::Trap(err))) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Trap(err))))
            }
            Poll::Ready(Err(StreamError::Closed)) => Poll::Ready(None),
        }
    }
}

#[instrument(level = "trace", skip(store))]
pub fn to_wrpc_value<T: WasiView>(
    mut store: impl AsContextMut<Data = T>,
    v: &Val,
    ty: &Type,
) -> anyhow::Result<wrpc_transport::Value> {
    let mut store = store.as_context_mut();
    match (v, ty) {
        (Val::Bool(v), Type::Bool) => Ok(wrpc_transport::Value::Bool(*v)),
        (Val::S8(v), Type::S8) => Ok(wrpc_transport::Value::S8(*v)),
        (Val::U8(v), Type::U8) => Ok(wrpc_transport::Value::U8(*v)),
        (Val::S16(v), Type::S16) => Ok(wrpc_transport::Value::S16(*v)),
        (Val::U16(v), Type::U16) => Ok(wrpc_transport::Value::U16(*v)),
        (Val::S32(v), Type::S32) => Ok(wrpc_transport::Value::S32(*v)),
        (Val::U32(v), Type::U32) => Ok(wrpc_transport::Value::U32(*v)),
        (Val::S64(v), Type::S64) => Ok(wrpc_transport::Value::S64(*v)),
        (Val::U64(v), Type::U64) => Ok(wrpc_transport::Value::U64(*v)),
        (Val::Float32(v), Type::Float32) => Ok(wrpc_transport::Value::F32(*v)),
        (Val::Float64(v), Type::Float64) => Ok(wrpc_transport::Value::F64(*v)),
        (Val::Char(v), Type::Char) => Ok(wrpc_transport::Value::Char(*v)),
        (Val::String(v), Type::String) => Ok(wrpc_transport::Value::String(v.to_string())),
        (Val::List(vs), Type::List(ty)) => {
            let ty = ty.ty();
            vs.iter()
                .map(|v| to_wrpc_value(&mut store, v, &ty))
                .collect::<anyhow::Result<_>>()
                .map(wrpc_transport::Value::List)
        }
        (Val::Record(vs), Type::Record(ty)) => zip(vs, ty.fields())
            .map(|((_, v), Field { ty, .. })| to_wrpc_value(&mut store, v, &ty))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport::Value::Record),
        (Val::Tuple(vs), Type::Tuple(ty)) => zip(vs, ty.types())
            .map(|(v, ty)| to_wrpc_value(&mut store, v, &ty))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport::Value::Tuple),
        (Val::Variant(discriminant, v), Type::Variant(ty)) => {
            let (discriminant, ty) = zip(0.., ty.cases())
                .find_map(|(i, Case { name, ty })| (name == discriminant).then_some((i, ty)))
                .context("unknown variant discriminant")?;
            let nested = match (v, ty) {
                (Some(v), Some(ty)) => {
                    let v = to_wrpc_value(store, v, &ty)?;
                    Some(Box::new(v))
                }
                (Some(_v), None) => {
                    bail!("variant value of unknown type")
                }
                (None, Some(_ty)) => {
                    bail!("variant value missing")
                }
                (None, None) => None,
            };
            Ok(wrpc_transport::Value::Variant {
                discriminant,
                nested,
            })
        }
        (Val::Enum(discriminant), Type::Enum(ty)) => zip(0.., ty.names())
            .find_map(|(i, name)| (name == discriminant).then_some(i))
            .context("unknown enum discriminant")
            .map(wrpc_transport::Value::Enum),
        (Val::Option(v), Type::Option(ty)) => v
            .as_ref()
            .map(|v| to_wrpc_value(store, v, &ty.ty()).map(Box::new))
            .transpose()
            .map(wrpc_transport::Value::Option),
        (Val::Result(v), Type::Result(ty)) => {
            let v = match v {
                Ok(v) => match (v, ty.ok()) {
                    (Some(v), Some(ty)) => {
                        let v = to_wrpc_value(store, v, &ty)?;
                        Ok(Some(Box::new(v)))
                    }
                    (Some(_v), None) => bail!("`result::ok` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::ok` value missing"),
                    (None, None) => Ok(None),
                },
                Err(v) => match (v, ty.err()) {
                    (Some(v), Some(ty)) => {
                        let v = to_wrpc_value(store, v, &ty)?;
                        Err(Some(Box::new(v)))
                    }
                    (Some(_v), None) => bail!("`result::err` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::err` value missing"),
                    (None, None) => Err(None),
                },
            };
            Ok(wrpc_transport::Value::Result(v))
        }
        (Val::Flags(vs), Type::Flags(ty)) => {
            let mut v = 0;
            for name in vs {
                let i = zip(0.., ty.names())
                    .find_map(|(i, flag_name)| (name == flag_name).then_some(i))
                    .context("unknown flag")?;
                ensure!(
                    i < 64,
                    "flag discriminants over 64 currently cannot be represented"
                );
                v |= 1 << i;
            }
            Ok(wrpc_transport::Value::Flags(v))
        }
        (Val::Resource(resource), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<InputStream>() {
                let stream = resource
                    .try_into_resource::<InputStream>(&mut store)
                    .context("failed to downcast `wasi:io/input-stream`")?;
                let stream = if stream.owned() {
                    store
                        .data_mut()
                        .table()
                        .delete(stream)
                        .context("failed to delete input stream")?
                } else {
                    store
                        .data_mut()
                        .table()
                        .get_mut(&stream)
                        .context("failed to get input stream")?;
                    // NOTE: In order to handle this we'd need to know how many bytes has the
                    // receiver read. That means that some kind of callback would be required from
                    // the receiver. This is not trivial and generally should be a very rare use case.
                    bail!("borrowed `wasi:io/input-stream` not supported yet");
                };
                Ok(wrpc_transport::Value::Stream(match stream {
                    InputStream::Host(stream) => {
                        Box::pin(OutgoingHostInputStream(stream).map(|buf| {
                            let buf = buf?;
                            Ok(buf
                                .into_iter()
                                .map(wrpc_transport::Value::U8)
                                .map(Some)
                                .collect())
                        }))
                    }
                    InputStream::File(stream) => {
                        Box::pin(OutgoingFileInputStream(stream).map(|buf| {
                            let buf = buf?;
                            Ok(buf
                                .into_iter()
                                .map(wrpc_transport::Value::U8)
                                .map(Some)
                                .collect())
                        }))
                    }
                }))
            } else if *ty == ResourceType::host::<Pollable>() {
                let pollable = resource
                    .try_into_resource::<Pollable>(&mut store)
                    .context("failed to downcast `wasi:io/pollable")?;
                if pollable.owned() {
                    store
                        .data_mut()
                        .table()
                        .delete(pollable)
                        .context("failed to delete pollable")?;
                } else {
                    store
                        .data_mut()
                        .table()
                        .get_mut(&pollable)
                        .context("failed to get pollable")?;
                };
                Ok(wrpc_transport::Value::Future(Box::pin(async {
                    bail!("`wasi:io/pollable` not supported yet")
                })))
            } else {
                bail!("resources not supported yet")
            }
        }
        _ => bail!("value type mismatch"),
    }
}

struct IncomingValueInputStream {
    stream: Pin<Box<dyn Stream<Item = anyhow::Result<Vec<Option<wrpc_transport::Value>>>> + Send>>,
    item: Option<Option<anyhow::Result<Vec<Option<wrpc_transport::Value>>>>>,
    buffer: Bytes,
}

#[async_trait]
impl Subscribe for IncomingValueInputStream {
    async fn ready(&mut self) {
        if self.item.is_some() || !self.buffer.is_empty() {
            return;
        }
        self.item = Some(self.stream.next().await);
    }
}

impl HostInputStream for IncomingValueInputStream {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if !self.buffer.is_empty() {
            if self.buffer.len() > size {
                return Ok(self.buffer.split_to(size));
            } else {
                return Ok(mem::take(&mut self.buffer));
            }
        }
        let Some(mut item) = self.item.take() else {
            // `ready` was not called yet
            return Ok(Bytes::default());
        };
        let Some(item) = item.take() else {
            // `next` returned `None`, assume stream is closed
            return Err(StreamError::Closed);
        };
        let values = item.map_err(StreamError::LastOperationFailed)?;
        let mut buffer = BytesMut::with_capacity(values.len());
        for value in values {
            let Some(wrpc_transport::Value::U8(v)) = value else {
                Err(StreamError::LastOperationFailed(anyhow!(
                    "stream item type mismatch"
                )))?
            };
            buffer.put_u8(v);
        }
        let buffer = buffer.freeze();
        if buffer.len() > size {
            self.buffer = buffer;
            Ok(self.buffer.split_to(size))
        } else {
            Ok(buffer)
        }
    }
}

#[instrument(level = "trace", skip(store, val))]
pub fn from_wrpc_value<T: WasiView>(
    mut store: impl AsContextMut<Data = T>,
    val: wrpc_transport::Value,
    ty: &Type,
) -> anyhow::Result<Val> {
    let mut store = store.as_context_mut();
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
        (wrpc_transport::Value::String(v), Type::String) => Ok(Val::String(v)),
        (wrpc_transport::Value::List(vs), Type::List(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            let el_ty = ty.ty();
            for v in vs {
                let v = from_wrpc_value(&mut store, v, &el_ty)
                    .context("failed to convert list element")?;
                w_vs.push(v);
            }
            Ok(Val::List(w_vs))
        }
        (wrpc_transport::Value::Record(vs), Type::Record(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, Field { name, ty }) in zip(vs, ty.fields()) {
                let v = from_wrpc_value(&mut store, v, &ty)
                    .context("failed to convert record field")?;
                w_vs.push((name.to_string(), v));
            }
            Ok(Val::Record(w_vs))
        }
        (wrpc_transport::Value::Tuple(vs), Type::Tuple(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, ty) in zip(vs, ty.types()) {
                let v = from_wrpc_value(&mut store, v, &ty)
                    .context("failed to convert tuple element")?;
                w_vs.push(v);
            }
            Ok(Val::Tuple(w_vs))
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
            let Case { name, ty } = ty
                .cases()
                .nth(discriminant)
                .context("variant discriminant not found")?;
            let v = if let Some(ty) = ty {
                let v = nested.context("nested value missing")?;
                let v =
                    from_wrpc_value(store, *v, &ty).context("failed to convert variant value")?;
                Some(Box::new(v))
            } else {
                None
            };
            Ok(Val::Variant(name.to_string(), v))
        }
        (wrpc_transport::Value::Enum(discriminant), Type::Enum(ty)) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            ty.names()
                .nth(discriminant)
                .context("enum discriminant not found")
                .map(ToString::to_string)
                .map(Val::Enum)
        }
        (wrpc_transport::Value::Option(v), Type::Option(ty)) => {
            let v = if let Some(v) = v {
                let v = from_wrpc_value(store, *v, &ty.ty())
                    .context("failed to convert option value")?;
                Some(Box::new(v))
            } else {
                None
            };
            Ok(Val::Option(v))
        }
        (wrpc_transport::Value::Result(v), Type::Result(ty)) => match v {
            Ok(None) => Ok(Val::Result(Ok(None))),
            Ok(Some(v)) => {
                let ty = ty.ok().context("`result::ok` type missing")?;
                let v = from_wrpc_value(store, *v, &ty)
                    .context("failed to convert `result::ok` value")?;
                Ok(Val::Result(Ok(Some(Box::new(v)))))
            }
            Err(None) => Ok(Val::Result(Err(None))),
            Err(Some(v)) => {
                let ty = ty.err().context("`result::err` type missing")?;
                let v = from_wrpc_value(store, *v, &ty)
                    .context("failed to convert `result::err` value")?;
                Ok(Val::Result(Err(Some(Box::new(v)))))
            }
        },
        (wrpc_transport::Value::Flags(v), Type::Flags(ty)) => {
            // NOTE: Currently flags are limited to 64
            let mut names = Vec::with_capacity(64);
            for (i, name) in zip(0..64, ty.names()) {
                if v & (1 << i) != 0 {
                    names.push(name.to_string());
                }
            }
            Ok(Val::Flags(names))
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
        (wrpc_transport::Value::Stream(v), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<InputStream>() {
                let res = store
                    .data_mut()
                    .table()
                    .push(InputStream::Host(Box::new(IncomingValueInputStream {
                        stream: v,
                        item: None,
                        buffer: Bytes::default(),
                    })))
                    .context("failed to push stream resource to table")?;
                res.try_into_resource_any(store)
                    .context("failed to convert resource to ResourceAny")
                    .map(Val::Resource)
            } else {
                // TODO: Implement in preview3 or via a wrpc-specific interface
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

pub struct ValEncoder<'a, T> {
    pub store: StoreContextMut<'a, T>,
    pub ty: &'a Type,
}

impl<T> ValEncoder<'_, T> {
    #[must_use]
    pub fn new<'a>(store: StoreContextMut<'a, T>, ty: &'a Type) -> ValEncoder<'a, T> {
        ValEncoder { store, ty }
    }

    pub fn with_type<'a>(&'a mut self, ty: &'a Type) -> ValEncoder<'a, T> {
        ValEncoder {
            store: self.store.as_context_mut(),
            ty,
        }
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
    cases: impl IntoIterator<Item = Case<'a>>,
    discriminant: &str,
) -> anyhow::Result<(T, Option<Type>)> {
    zip(iter, cases)
        .find_map(|(i, Case { name, ty })| (name == discriminant).then_some((i, ty)))
        .context("unknown variant discriminant")
}

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

impl<T> Encoder<&Val> for ValEncoder<'_, T>
where
    T: WasiView,
{
    type Error = wasmtime::Error;

    fn encode(&mut self, v: &Val, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match (v, self.ty) {
            (Val::Bool(v), Type::Bool) => {
                dst.reserve(1);
                dst.put_u8((*v).into());
                Ok(())
            }
            (Val::S8(v), Type::S8) => {
                dst.reserve(1);
                dst.put_i8(*v);
                Ok(())
            }
            (Val::U8(v), Type::U8) => {
                dst.reserve(1);
                dst.put_u8(*v);
                Ok(())
            }
            (Val::S16(v), Type::S16) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s16"),
            (Val::U16(v), Type::U16) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u16"),
            (Val::S32(v), Type::S32) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s32"),
            (Val::U32(v), Type::U32) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u32"),
            (Val::S64(v), Type::S64) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode s64"),
            (Val::U64(v), Type::U64) => Leb128Encoder
                .encode(*v, dst)
                .context("failed to encode u64"),
            (Val::Float32(v), Type::Float32) => {
                dst.reserve(4);
                dst.put_f32_le(*v);
                Ok(())
            }
            (Val::Float64(v), Type::Float64) => {
                dst.reserve(8);
                dst.put_f64_le(*v);
                Ok(())
            }
            (Val::Char(v), Type::Char) => {
                CharEncoder.encode(*v, dst).context("failed to encode char")
            }
            (Val::String(v), Type::String) => CoreStringEncoder
                .encode(v.as_str(), dst)
                .context("failed to encode string"),
            (Val::List(vs), Type::List(ty)) => {
                let ty = ty.ty();
                let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
                dst.reserve(5 + vs.len());
                Leb128Encoder
                    .encode(n, dst)
                    .context("failed to encode list length")?;
                for v in vs {
                    self.with_type(&ty)
                        .encode(v, dst)
                        .context("failed to encode list element")?;
                }
                Ok(())
            }
            (Val::Record(vs), Type::Record(ty)) => {
                dst.reserve(vs.len());
                for ((name, v), Field { ref ty, .. }) in zip(vs, ty.fields()) {
                    self.with_type(ty)
                        .encode(v, dst)
                        .with_context(|| format!("failed to encode `{name}` field"))?;
                }
                Ok(())
            }
            (Val::Tuple(vs), Type::Tuple(ty)) => {
                dst.reserve(vs.len());
                for (v, ref ty) in zip(vs, ty.types()) {
                    self.with_type(ty)
                        .encode(v, dst)
                        .context("failed to encode tuple element")?;
                }
                Ok(())
            }
            (Val::Variant(discriminant, v), Type::Variant(ty)) => {
                let cases = ty.cases();
                let ty = match cases.len() {
                    ..=0x0000_00ff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0.., cases, discriminant)?;
                        dst.reserve(1 + usize::from(v.is_some()));
                        dst.put_u8(discriminant);
                        ty
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0.., cases, discriminant)?;
                        dst.reserve(2 + usize::from(v.is_some()));
                        dst.put_u16_le(discriminant);
                        ty
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0.., cases, discriminant)?;
                        dst.reserve(3 + usize::from(v.is_some()));
                        dst.put_slice(&u32::to_le_bytes(discriminant)[..3]);
                        ty
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0.., cases, discriminant)?;
                        dst.reserve(4 + usize::from(v.is_some()));
                        dst.put_u32_le(discriminant);
                        ty
                    }
                    0x1_0000_0000.. => bail!("case count does not fit in u32"),
                };
                if let Some(v) = v {
                    let ty = ty.context("type missing for variant")?;
                    self.with_type(&ty)
                        .encode(v, dst)
                        .context("failed to encode variant value")
                } else {
                    Ok(())
                }
            }
            (Val::Enum(discriminant), Type::Enum(ty)) => {
                let names = ty.names();
                match names.len() {
                    ..=0x0000_00ff => {
                        let discriminant = find_enum_discriminant(0.., names, discriminant)?;
                        dst.reserve(1);
                        dst.put_u8(discriminant);
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let discriminant = find_enum_discriminant(0.., names, discriminant)?;
                        dst.reserve(2);
                        dst.put_u16_le(discriminant);
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let discriminant = find_enum_discriminant(0.., names, discriminant)?;
                        dst.reserve(3);
                        dst.put_slice(&u32::to_le_bytes(discriminant)[..3]);
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let discriminant = find_enum_discriminant(0.., names, discriminant)?;
                        dst.reserve(4);
                        dst.put_u32_le(discriminant);
                    }
                    0x1_0000_0000.. => bail!("name count does not fit in u32"),
                }
                Ok(())
            }
            (Val::Option(None), Type::Option(_)) => {
                dst.reserve(1);
                dst.put_u8(0);
                Ok(())
            }
            (Val::Option(Some(v)), Type::Option(ty)) => {
                dst.reserve(2);
                dst.put_u8(1);
                self.with_type(&ty.ty()).encode(v, dst)
            }
            (Val::Result(v), Type::Result(ty)) => match v {
                Ok(v) => match (v, ty.ok()) {
                    (Some(v), Some(ty)) => {
                        dst.reserve(2);
                        dst.put_u8(0);
                        self.with_type(&ty).encode(v, dst)
                    }
                    (Some(_v), None) => bail!("`result::ok` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::ok` value missing"),
                    (None, None) => {
                        dst.reserve(1);
                        dst.put_u8(0);
                        Ok(())
                    }
                },
                Err(v) => match (v, ty.err()) {
                    (Some(v), Some(ty)) => {
                        dst.reserve(2);
                        dst.put_u8(1);
                        self.with_type(&ty).encode(v, dst)
                    }
                    (Some(_v), None) => bail!("`result::err` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::err` value missing"),
                    (None, None) => {
                        dst.reserve(1);
                        dst.put_u8(1);
                        Ok(())
                    }
                },
            },
            (Val::Flags(vs), Type::Flags(ty)) => {
                let names = ty.names();
                let vs = vs.iter().map(String::as_str);
                match names.len() {
                    ..=8 => {
                        dst.reserve(1);
                        dst.put_u8(flag_bits(names, vs));
                    }
                    9..=16 => {
                        dst.reserve(2);
                        dst.put_u16_le(flag_bits(names, vs));
                    }
                    17..=24 => {
                        dst.reserve(3);
                        dst.put_slice(&u32::to_le_bytes(flag_bits(names, vs))[..3]);
                    }
                    25..=32 => {
                        dst.reserve(4);
                        dst.put_u32_le(flag_bits(names, vs));
                    }
                    33..=40 => {
                        dst.reserve(5);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..5]);
                    }
                    41..=48 => {
                        dst.reserve(6);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..6]);
                    }
                    49..=56 => {
                        dst.reserve(7);
                        dst.put_slice(&u64::to_le_bytes(flag_bits(names, vs))[..7]);
                    }
                    57..=64 => {
                        dst.reserve(8);
                        dst.put_u64_le(flag_bits(names, vs));
                    }
                    65..=72 => {
                        dst.reserve(9);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..9]);
                    }
                    73..=80 => {
                        dst.reserve(10);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..10]);
                    }
                    81..=88 => {
                        dst.reserve(11);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..11]);
                    }
                    89..=96 => {
                        dst.reserve(12);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..12]);
                    }
                    97..=104 => {
                        dst.reserve(13);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..13]);
                    }
                    105..=112 => {
                        dst.reserve(14);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..14]);
                    }
                    113..=120 => {
                        dst.reserve(15);
                        dst.put_slice(&u128::to_le_bytes(flag_bits(names, vs))[..15]);
                    }
                    121..=128 => {
                        dst.reserve(16);
                        dst.put_u128_le(flag_bits(names, vs));
                    }
                    129.. => bail!("flags do not fit in u128"),
                }
                Ok(())
            }
            (Val::Resource(resource), Type::Own(_) | Type::Borrow(_)) => {
                if resource.ty() == ResourceType::host::<RemoteResource>() {
                    let resource = resource
                        .try_into_resource(&mut self.store)
                        .context("resource type mismatch")?;
                    let table = self.store.data_mut().table();
                    if resource.owned() {
                        let RemoteResource(id) = table
                            .delete(resource)
                            .context("failed to delete remote resource")?;
                        CoreStringEncoder
                            .encode(id, dst)
                            .context("failed to encode resource ID")
                    } else {
                        let RemoteResource(id) = table
                            .get(&resource)
                            .context("failed to get remote resource")?;
                        CoreStringEncoder
                            .encode(id.as_str(), dst)
                            .context("failed to encode resource ID")
                    }
                } else {
                    bail!("encoding host resources not supported yet")
                }
            }
            _ => bail!("value type mismatch"),
        }
    }
}

async fn read_discriminant(
    cases: usize,
    r: &mut (impl AsyncRead + Unpin),
) -> std::io::Result<usize> {
    match cases {
        ..=0x0000_00ff => r.read_u8().await.map(Into::into),
        0x0000_0100..=0x0000_ffff => r.read_u16_le().await.map(Into::into),
        0x0001_0000..=0x00ff_ffff => {
            let mut buf = 0usize.to_le_bytes();
            r.read_exact(&mut buf[..3]).await?;
            Ok(usize::from_le_bytes(buf))
        }
        0x0100_0000..=0xffff_ffff => {
            let discriminant = r.read_u32_le().await?;
            discriminant
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
        }
        0x1_0000_0000.. => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "case count does not fit in u32".to_string(),
            ))
        }
    }
}

#[inline]
async fn read_flags(n: usize, r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<u128> {
    let mut buf = 0u128.to_le_bytes();
    r.read_exact(&mut buf[..n]).await?;
    Ok(u128::from_le_bytes(buf))
}

pub async fn read_value<T: WasiView>(
    store: &mut impl AsContextMut<Data = T>,
    r: &mut (impl AsyncRead + Unpin),
    val: &mut Val,
    ty: &Type,
) -> std::io::Result<()> {
    match ty {
        Type::Bool => {
            let v = r.read_bool().await?;
            *val = Val::Bool(v);
            Ok(())
        }
        Type::S8 => {
            let v = r.read_i8().await?;
            *val = Val::S8(v);
            Ok(())
        }
        Type::U8 => {
            let v = r.read_u8().await?;
            *val = Val::U8(v);
            Ok(())
        }
        Type::S16 => {
            let v = r.read_i16_leb128().await?;
            *val = Val::S16(v);
            Ok(())
        }
        Type::U16 => {
            let v = r.read_u16_leb128().await?;
            *val = Val::U16(v);
            Ok(())
        }
        Type::S32 => {
            let v = r.read_i32_leb128().await?;
            *val = Val::S32(v);
            Ok(())
        }
        Type::U32 => {
            let v = r.read_u32_leb128().await?;
            *val = Val::U32(v);
            Ok(())
        }
        Type::S64 => {
            let v = r.read_i64_leb128().await?;
            *val = Val::S64(v);
            Ok(())
        }
        Type::U64 => {
            let v = r.read_u64_leb128().await?;
            *val = Val::U64(v);
            Ok(())
        }
        Type::Float32 => {
            let v = r.read_f32_le().await?;
            *val = Val::Float32(v);
            Ok(())
        }
        Type::Float64 => {
            let v = r.read_f64_le().await?;
            *val = Val::Float64(v);
            Ok(())
        }
        Type::Char => {
            let v = r.read_char().await?;
            *val = Val::Char(v);
            Ok(())
        }
        Type::String => {
            let mut s = String::default();
            r.read_core_string(&mut s).await?;
            *val = Val::String(s);
            Ok(())
        }
        Type::List(ty) => {
            let n = r.read_u32_leb128().await?;
            let n = n
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            let mut vs = Vec::with_capacity(n);
            let ty = ty.ty();
            for _ in 0..n {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty)).await?;
                vs.push(v);
            }
            *val = Val::List(vs);
            Ok(())
        }
        Type::Record(ty) => {
            let fields = ty.fields();
            let mut vs = Vec::with_capacity(fields.len());
            for Field { name, ty } in fields {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty)).await?;
                vs.push((name.to_string(), v));
            }
            *val = Val::Record(vs);
            Ok(())
        }
        Type::Tuple(ty) => {
            let types = ty.types();
            let mut vs = Vec::with_capacity(types.len());
            for ty in types {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty)).await?;
                vs.push(v);
            }
            *val = Val::Tuple(vs);
            Ok(())
        }
        Type::Variant(ty) => {
            let mut cases = ty.cases();
            let discriminant = read_discriminant(cases.len(), r).await?;
            let Case { name, ty } = cases.nth(discriminant).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown variant discriminant `{discriminant}`"),
                )
            })?;
            let name = name.to_string();
            if let Some(ty) = ty {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty)).await?;
                *val = Val::Variant(name, Some(Box::new(v)));
            } else {
                *val = Val::Variant(name, None);
            }
            Ok(())
        }
        Type::Enum(ty) => {
            let mut names = ty.names();
            let discriminant = read_discriminant(names.len(), r).await?;
            let name = names.nth(discriminant).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown enum discriminant `{discriminant}`"),
                )
            })?;
            *val = Val::Enum(name.to_string());
            Ok(())
        }
        Type::Option(ty) => {
            let ok = r.read_option_status().await?;
            if ok {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty.ty())).await?;
                *val = Val::Option(Some(Box::new(v)));
            } else {
                *val = Val::Option(None);
            }
            Ok(())
        }
        Type::Result(ty) => {
            let ok = r.read_result_status().await?;
            if ok {
                if let Some(ty) = ty.ok() {
                    let mut v = Val::Bool(false);
                    Box::pin(read_value(store, r, &mut v, &ty)).await?;
                    *val = Val::Result(Ok(Some(Box::new(v))));
                } else {
                    *val = Val::Result(Ok(None));
                }
            } else if let Some(ty) = ty.err() {
                let mut v = Val::Bool(false);
                Box::pin(read_value(store, r, &mut v, &ty)).await?;
                *val = Val::Result(Err(Some(Box::new(v))));
            } else {
                *val = Val::Result(Err(None));
            }
            Ok(())
        }
        Type::Flags(ty) => {
            let names = ty.names();
            let flags = match names.len() {
                ..=8 => read_flags(1, r).await?,
                9..=16 => read_flags(2, r).await?,
                17..=24 => read_flags(3, r).await?,
                25..=32 => read_flags(4, r).await?,
                33..=40 => read_flags(5, r).await?,
                41..=48 => read_flags(6, r).await?,
                49..=56 => read_flags(7, r).await?,
                57..=64 => read_flags(8, r).await?,
                65..=72 => read_flags(9, r).await?,
                73..=80 => read_flags(10, r).await?,
                81..=88 => read_flags(11, r).await?,
                89..=96 => read_flags(12, r).await?,
                97..=104 => read_flags(13, r).await?,
                105..=112 => read_flags(14, r).await?,
                113..=120 => read_flags(15, r).await?,
                121..=128 => r.read_u128_le().await?,
                129.. => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "flags do not fit in u128".to_string(),
                    ))
                }
            };
            let mut vs = Vec::with_capacity(flags.count_ones().try_into().unwrap_or(usize::MAX));
            for (i, name) in zip(0.., names) {
                if flags & (1 << i) > 0 {
                    vs.push(name.to_string());
                }
            }
            *val = Val::Flags(vs);
            Ok(())
        }
        Type::Own(_) | Type::Borrow(_) => {
            let mut store = store.as_context_mut();
            let mut s = String::default();
            r.read_core_string(&mut s).await?;
            let table = store.data_mut().table();
            let resource = table
                .push(RemoteResource(s))
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
            let resource = resource
                .try_into_resource_any(store)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
            *val = Val::Resource(resource);
            Ok(())
        }
    }
}

pub trait WrpcView<C: Invoke<Error = wasmtime::Error>>: Send {
    fn client(&self) -> &C;
}

/// Polyfills all missing imports
#[instrument(level = "trace", skip_all)]
pub fn polyfill<'a, T, C, V>(
    resolve: &wit_parser::Resolve,
    imports: T,
    engine: &wasmtime::Engine,
    ty: &types::Component,
    linker: &mut Linker<V>,
    cx: C::Context,
) where
    T: IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
    T::IntoIter: ExactSizeIterator,
    V: WrpcView<C> + WasiView,
    C: Invoke<Error = wasmtime::Error>,
    C::Context: Clone + 'static,
    C::Incoming: Sized,
    C::Session: Session<TransportError = wasmtime::Error>,
{
    let imports = imports.into_iter();
    for (wk, item) in imports {
        let instance_name = resolve.name_world_key(wk);
        // Avoid polyfilling instances, for which static bindings are linked
        match instance_name.as_ref() {
            "wasi:cli/environment@0.2.0"
            | "wasi:cli/exit@0.2.0"
            | "wasi:cli/stderr@0.2.0"
            | "wasi:cli/stdin@0.2.0"
            | "wasi:cli/stdout@0.2.0"
            | "wasi:cli/terminal-input@0.2.0"
            | "wasi:cli/terminal-output@0.2.0"
            | "wasi:cli/terminal-stderr@0.2.0"
            | "wasi:cli/terminal-stdin@0.2.0"
            | "wasi:cli/terminal-stdout@0.2.0"
            | "wasi:clocks/monotonic-clock@0.2.0"
            | "wasi:clocks/wall-clock@0.2.0"
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:keyvalue/store@0.2.0-draft"
            | "wasi:random/random@0.2.0"
            | "wasi:sockets/instance-network@0.2.0"
            | "wasi:sockets/network@0.2.0"
            | "wasi:sockets/tcp-create-socket@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasi:sockets/udp-create-socket@0.2.0"
            | "wasi:sockets/udp@0.2.0" => continue,
            _ => {}
        }
        let wit_parser::WorldItem::Interface(interface) = item else {
            continue;
        };
        let Some(wit_parser::Interface {
            functions, types, ..
        }) = resolve.interfaces.get(*interface)
        else {
            warn!("component imports a non-existent interface");
            continue;
        };
        let Some(types::ComponentItem::ComponentInstance(instance)) =
            ty.get_import(engine, &instance_name)
        else {
            trace!(
                instance_name,
                "component does not import the parsed instance"
            );
            continue;
        };

        let mut linker = linker.root();
        let mut linker = match linker.instance(&instance_name) {
            Ok(linker) => linker,
            Err(err) => {
                error!(
                    ?err,
                    ?instance_name,
                    "failed to instantiate interface from root"
                );
                continue;
            }
        };
        for (name, _) in types {
            let Some(types::ComponentItem::Resource(_)) = instance.get_export(engine, name) else {
                trace!(?instance_name, name, "skip non-resource type import");
                continue;
            };
            if let Err(err) =
                linker.resource(name, ResourceType::host::<RemoteResource>(), |_, _| Ok(()))
            {
                error!(?err, "failed to polyfill imported resource type")
            }
        }
        let instance_name = Arc::new(instance_name);
        for (func_name, ty) in functions {
            trace!(
                ?instance_name,
                func_name,
                "polyfill component function import"
            );
            let Some(types::ComponentItem::ComponentFunc(func)) =
                instance.get_export(engine, func_name)
            else {
                trace!(
                    ?instance_name,
                    func_name,
                    "instance does not export the parsed function"
                );
                continue;
            };
            let cx = cx.clone();
            let instance_name = Arc::clone(&instance_name);
            let func_name = Arc::new(func_name.to_string());
            let rpc_name = Arc::new(rpc_func_name(ty).to_string());
            if let Err(err) = match ty.kind {
                FunctionKind::Method(..) => linker.func_new_async(
                    Arc::clone(&func_name).as_str(),
                    move |mut store, params, results| {
                        let cx = cx.clone();
                        let func = func.clone();
                        let rpc_name = Arc::clone(&rpc_name);
                        let func_name = Arc::clone(&func_name);
                        Box::new(async move {
                            let (this, params) = params.split_first().context("method receiver missing")?;
                            let Val::Resource(this) = this else {
                                bail!("first method parameter is not a resource")
                            };
                            let this = this
                                .try_into_resource(&mut store)
                                .context("resource type mismatch")?;
                            let mut buf = BytesMut::default();
                            for (v, ref ty) in zip(params, func.params()) {
                                ValEncoder::new(store.as_context_mut(), ty).encode(v, &mut buf).context("failed to encode parameter")?;
                            }
                            let RemoteResource(this) = store
                                .data_mut()
                                .table()
                                .get(&this)
                                .context("failed to get remote resource")?;
                            let this = this.to_string();
                            let Invocation { outgoing, incoming, session } = store
                                .data()
                                .client()
                                .invoke(cx, &this, &rpc_name, buf.freeze(), &[])
                                .await
                                .with_context(|| {
                                    format!("failed to invoke `{this}.{func_name}` polyfill via wRPC")
                                })?;
                            try_join!(
                                async {
                                    pin!(outgoing).shutdown().await.context("failed to shutdown outgoing stream")
                                },
                                async {
                                    let mut incoming = pin!(incoming);
                                    for (v, ref ty) in zip(results, func.results()) {
                                        read_value(&mut store, &mut incoming, v, ty).await.context("failed to decode result value")?;
                                    }
                                    Ok(())
                                },
                            )?;
                            match session.finish(Ok(())).await? {
                                Ok(()) => Ok(()),
                                Err(err) => bail!(anyhow!("{err}").context("session failed"))
                            }
                        })
                    },
                ),
                FunctionKind::Freestanding | FunctionKind::Static(_) | FunctionKind::Constructor(..) => linker.func_new_async(
                    Arc::clone(&func_name).as_str(),
                    move |mut store, params, results| {
                        let cx = cx.clone();
                        let instance_name = Arc::clone(&instance_name);
                        let func = func.clone();
                        let rpc_name = Arc::clone(&rpc_name);
                        let func_name = Arc::clone(&func_name);
                        Box::new(async move {
                            let mut buf = BytesMut::default();
                            for (v, ref ty) in zip(params, func.params()) {
                                ValEncoder::new(store.as_context_mut(), ty).encode(v, &mut buf).context("failed to encode parameter")?;
                            }
                            let Invocation { outgoing, incoming, session } = store
                                .data()
                                .client()
                                .invoke(cx, &instance_name, &rpc_name, buf.freeze(), &[])
                                .await
                                .with_context(|| {
                                    format!("failed to invoke `{instance_name}.{func_name}` polyfill via wRPC")
                                })?;
                            try_join!(
                                async {
                                    pin!(outgoing).shutdown().await.context("failed to shutdown outgoing stream")
                                },
                                async {
                                    let mut incoming = pin!(incoming);
                                    for (v, ref ty) in zip(results, func.results()) {
                                        read_value(&mut store, &mut incoming, v, ty).await.context("failed to decode result value")?;
                                    }
                                    Ok(())
                                },
                            )?;
                            match session.finish(Ok(())).await? {
                                Ok(()) => Ok(()),
                                Err(err) => bail!(anyhow!("{err}").context("session failed"))
                            }
                        })
                    },
                ),
            } {
                error!(?err, "failed to polyfill component function import");
            }
        }
    }
}
