use core::future::Future;
use core::iter::zip;
use core::ops::{BitOrAssign, Shl};
use core::pin::{pin, Pin};

use std::collections::HashSet;

use anyhow::{bail, Context as _};
use bytes::{BufMut as _, BytesMut};
use futures::stream::FuturesUnordered;
use futures::TryStreamExt as _;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::{Encoder, FramedRead};
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tracing::{error, instrument, trace, warn};
use uuid::Uuid;
use wasm_tokio::cm::AsyncReadValue as _;
use wasm_tokio::{
    AsyncReadCore as _, AsyncReadLeb128 as _, AsyncReadUtf8 as _, CoreNameEncoder,
    CoreVecEncoderBytes, Leb128Encoder, Utf8Codec,
};
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{ResourceType, Type, Val};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::p2::pipe::AsyncReadStream;
use wasmtime_wasi::p2::{DynInputStream, StreamError, WasiView};
use wrpc_transport::ListDecoderU8;

use crate::{RemoteResource, WrpcView};

pub struct ValEncoder<'a, T, W> {
    pub store: StoreContextMut<'a, T>,
    pub ty: &'a Type,
    pub resources: &'a [ResourceType],
    pub deferred: Option<
        Box<dyn FnOnce(W) -> Pin<Box<dyn Future<Output = wasmtime::Result<()>> + Send>> + Send>,
    >,
}

impl<T, W> ValEncoder<'_, T, W> {
    #[must_use]
    pub fn new<'a>(
        store: StoreContextMut<'a, T>,
        ty: &'a Type,
        resources: &'a [ResourceType],
    ) -> ValEncoder<'a, T, W> {
        ValEncoder {
            store,
            ty,
            resources,
            deferred: None,
        }
    }

    pub fn with_type<'a>(&'a mut self, ty: &'a Type) -> ValEncoder<'a, T, W> {
        ValEncoder {
            store: self.store.as_context_mut(),
            ty,
            resources: self.resources,
            deferred: None,
        }
    }
}

fn find_enum_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    names: impl IntoIterator<Item = &'a str>,
    discriminant: &str,
) -> wasmtime::Result<T> {
    zip(iter, names)
        .find_map(|(i, name)| (name == discriminant).then_some(i))
        .context("unknown enum discriminant")
}

fn find_variant_discriminant<'a, T>(
    iter: impl IntoIterator<Item = T>,
    cases: impl IntoIterator<Item = Case<'a>>,
    discriminant: &str,
) -> wasmtime::Result<(T, Option<Type>)> {
    zip(iter, cases)
        .find_map(|(i, Case { name, ty })| (name == discriminant).then_some((i, ty)))
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

async fn write_deferred<W, I>(w: W, deferred: I) -> wasmtime::Result<()>
where
    W: wrpc_transport::Index<W> + Sync + Send + 'static,
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator<
        Item = Option<
            Box<dyn FnOnce(W) -> Pin<Box<dyn Future<Output = wasmtime::Result<()>> + Send>> + Send>,
        >,
    >,
{
    let mut futs: FuturesUnordered<_> = zip(0.., deferred)
        .filter_map(|(i, f)| f.map(|f| (w.index(&[i]), f)))
        .map(|(w, f)| async move {
            let w = w?;
            f(w).await
        })
        .collect();
    while let Some(()) = futs.try_next().await? {}
    Ok(())
}

impl<T, W> Encoder<&Val> for ValEncoder<'_, T, W>
where
    T: WasiView + WrpcView,
    W: AsyncWrite + wrpc_transport::Index<W> + Sync + Send + 'static,
{
    type Error = wasmtime::Error;

    #[allow(clippy::too_many_lines)]
    #[instrument(level = "trace", skip(self))]
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
                Utf8Codec.encode(*v, dst).context("failed to encode char")
            }
            (Val::String(v), Type::String) => CoreNameEncoder
                .encode(v.as_str(), dst)
                .context("failed to encode string"),
            (Val::List(vs), Type::List(ty)) => {
                let ty = ty.ty();
                let n = u32::try_from(vs.len()).context("list length does not fit in u32")?;
                dst.reserve(5 + vs.len());
                Leb128Encoder
                    .encode(n, dst)
                    .context("failed to encode list length")?;
                let mut deferred = Vec::with_capacity(vs.len());
                for v in vs {
                    let mut enc = self.with_type(&ty);
                    enc.encode(v, dst)
                        .context("failed to encode list element")?;
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
                }
                Ok(())
            }
            (Val::Record(vs), Type::Record(ty)) => {
                dst.reserve(vs.len());
                let mut deferred = Vec::with_capacity(vs.len());
                for ((name, v), Field { ref ty, .. }) in zip(vs, ty.fields()) {
                    let mut enc = self.with_type(ty);
                    enc.encode(v, dst)
                        .with_context(|| format!("failed to encode `{name}` field"))?;
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
                }
                Ok(())
            }
            (Val::Tuple(vs), Type::Tuple(ty)) => {
                dst.reserve(vs.len());
                let mut deferred = Vec::with_capacity(vs.len());
                for (v, ref ty) in zip(vs, ty.types()) {
                    let mut enc = self.with_type(ty);
                    enc.encode(v, dst)
                        .context("failed to encode tuple element")?;
                    deferred.push(enc.deferred);
                }
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|w| Box::pin(write_deferred(w, deferred))));
                }
                Ok(())
            }
            (Val::Variant(discriminant, v), Type::Variant(ty)) => {
                let cases = ty.cases();
                let ty = match cases.len() {
                    ..=0x0000_00ff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u8.., cases, discriminant)?;
                        dst.reserve(2 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u16.., cases, discriminant)?;
                        dst.reserve(3 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u32.., cases, discriminant)?;
                        dst.reserve(4 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let (discriminant, ty) =
                            find_variant_discriminant(0u32.., cases, discriminant)?;
                        dst.reserve(5 + usize::from(v.is_some()));
                        Leb128Encoder.encode(discriminant, dst)?;
                        ty
                    }
                    0x1_0000_0000.. => bail!("case count does not fit in u32"),
                };
                if let Some(v) = v {
                    let ty = ty.context("type missing for variant")?;
                    let mut enc = self.with_type(&ty);
                    enc.encode(v, dst)
                        .context("failed to encode variant value")?;
                    if let Some(f) = enc.deferred {
                        self.deferred = Some(f);
                    }
                }
                Ok(())
            }
            (Val::Enum(discriminant), Type::Enum(ty)) => {
                let names = ty.names();
                match names.len() {
                    ..=0x0000_00ff => {
                        let discriminant = find_enum_discriminant(0u8.., names, discriminant)?;
                        dst.reserve(2);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0000_0100..=0x0000_ffff => {
                        let discriminant = find_enum_discriminant(0u16.., names, discriminant)?;
                        dst.reserve(3);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0001_0000..=0x00ff_ffff => {
                        let discriminant = find_enum_discriminant(0u32.., names, discriminant)?;
                        dst.reserve(4);
                        Leb128Encoder.encode(discriminant, dst)?;
                    }
                    0x0100_0000..=0xffff_ffff => {
                        let discriminant = find_enum_discriminant(0u32.., names, discriminant)?;
                        dst.reserve(5);
                        Leb128Encoder.encode(discriminant, dst)?;
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
                let ty = ty.ty();
                let mut enc = self.with_type(&ty);
                enc.encode(v, dst)
                    .context("failed to encode `option::some` value")?;
                if let Some(f) = enc.deferred {
                    self.deferred = Some(f);
                }
                Ok(())
            }
            (Val::Result(v), Type::Result(ty)) => match v {
                Ok(v) => match (v, ty.ok()) {
                    (Some(v), Some(ty)) => {
                        dst.reserve(2);
                        dst.put_u8(0);
                        let mut enc = self.with_type(&ty);
                        enc.encode(v, dst)
                            .context("failed to encode `result::ok` value")?;
                        if let Some(f) = enc.deferred {
                            self.deferred = Some(f);
                        }
                        Ok(())
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
                        let mut enc = self.with_type(&ty);
                        enc.encode(v, dst)
                            .context("failed to encode `result::err` value")?;
                        if let Some(f) = enc.deferred {
                            self.deferred = Some(f);
                        }
                        Ok(())
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
                    bits @ 129.. => {
                        let mut cap = bits / 8;
                        if bits % 8 != 0 {
                            cap = cap.saturating_add(1);
                        }
                        let mut buf = vec![0; cap];
                        let flags: HashSet<&str> = vs.into_iter().collect();
                        for (i, name) in names.enumerate() {
                            if flags.contains(name) {
                                buf[i / 8] |= 1 << (i % 8);
                            }
                        }
                        dst.extend_from_slice(&buf);
                    }
                }
                Ok(())
            }
            (Val::Resource(resource), Type::Own(ty) | Type::Borrow(ty)) => {
                if *ty == ResourceType::host::<DynInputStream>() {
                    let stream = resource
                        .try_into_resource::<DynInputStream>(&mut self.store)
                        .context("failed to downcast `wasi:io/input-stream`")?;
                    if stream.owned() {
                        let mut stream = self
                            .store
                            .data_mut()
                            .table()
                            .delete(stream)
                            .context("failed to delete input stream")?;
                        self.deferred = Some(Box::new(|w| {
                            Box::pin(async move {
                                let mut w = pin!(w);
                                loop {
                                    stream.ready().await;
                                    match stream.read(8096) {
                                        Ok(buf) => {
                                            let mut chunk = BytesMut::with_capacity(
                                                buf.len().saturating_add(5),
                                            );
                                            CoreVecEncoderBytes
                                                .encode(buf, &mut chunk)
                                                .context("failed to encode input stream chunk")?;
                                            w.write_all(&chunk).await?;
                                        }
                                        Err(StreamError::Closed) => {
                                            w.write_all(&[0x00]).await?;
                                        }
                                        Err(err) => return Err(err.into()),
                                    }
                                }
                            })
                        }));
                    } else {
                        self.store
                            .data_mut()
                            .table()
                            .get_mut(&stream)
                            .context("failed to get input stream")?;
                        // NOTE: In order to handle this we'd need to know how many bytes the
                        // receiver has read. That means that some kind of callback would be required from
                        // the receiver. This is not trivial and generally should be a very rare use case.
                        bail!("encoding borrowed `wasi:io/input-stream` not supported yet");
                    };
                    Ok(())
                } else if resource.ty() == ResourceType::host::<RemoteResource>() {
                    let resource = resource
                        .try_into_resource(&mut self.store)
                        .context("resource type mismatch")?;
                    let table = self.store.data_mut().table();
                    if resource.owned() {
                        let RemoteResource(buf) = table
                            .delete(resource)
                            .context("failed to delete remote resource")?;
                        CoreVecEncoderBytes
                            .encode(buf, dst)
                            .context("failed to encode resource handle")
                    } else {
                        let RemoteResource(buf) = table
                            .get(&resource)
                            .context("failed to get remote resource")?;
                        CoreVecEncoderBytes
                            .encode(buf, dst)
                            .context("failed to encode resource handle")
                    }
                } else if self.resources.contains(ty) {
                    let id = Uuid::now_v7();
                    CoreVecEncoderBytes
                        .encode(id.to_bytes_le().as_slice(), dst)
                        .context("failed to encode resource handle")?;
                    trace!(?id, "store shared resource");
                    if self
                        .store
                        .data_mut()
                        .shared_resources()
                        .0
                        .insert(id, *resource)
                        .is_some()
                    {
                        error!(?id, "duplicate resource ID generated");
                    }
                    Ok(())
                } else {
                    bail!("encoding host resources not supported yet")
                }
            }
            _ => bail!("value type mismatch"),
        }
    }
}

#[inline]
async fn read_flags(n: usize, r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<u128> {
    let mut buf = 0u128.to_le_bytes();
    r.read_exact(&mut buf[..n]).await?;
    Ok(u128::from_le_bytes(buf))
}

/// Read encoded value of type [`Type`] from an [`AsyncRead`] into a [`Val`]
#[instrument(level = "trace", skip_all, fields(ty, path))]
pub async fn read_value<T, R>(
    store: &mut impl AsContextMut<Data = T>,
    r: &mut Pin<&mut R>,
    resources: &[ResourceType],
    val: &mut Val,
    ty: &Type,
    path: &[usize],
) -> std::io::Result<()>
where
    T: WasiView + WrpcView,
    R: AsyncRead + wrpc_transport::Index<R> + Send + Unpin + 'static,
{
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
            let v = r.read_char_utf8().await?;
            *val = Val::Char(v);
            Ok(())
        }
        Type::String => {
            let mut s = String::default();
            r.read_core_name(&mut s).await?;
            *val = Val::String(s);
            Ok(())
        }
        Type::List(ty) => {
            let n = r.read_u32_leb128().await?;
            let n = n.try_into().unwrap_or(usize::MAX);
            let mut vs = Vec::with_capacity(n);
            let ty = ty.ty();
            let mut path = path.to_vec();
            for i in 0..n {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading list element value");
                Box::pin(read_value(store, r, resources, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push(v);
            }
            *val = Val::List(vs);
            Ok(())
        }
        Type::Record(ty) => {
            let fields = ty.fields();
            let mut vs = Vec::with_capacity(fields.len());
            let mut path = path.to_vec();
            for (i, Field { name, ty }) in fields.enumerate() {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading struct field value");
                Box::pin(read_value(store, r, resources, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push((name.to_string(), v));
            }
            *val = Val::Record(vs);
            Ok(())
        }
        Type::Tuple(ty) => {
            let types = ty.types();
            let mut vs = Vec::with_capacity(types.len());
            let mut path = path.to_vec();
            for (i, ty) in types.enumerate() {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading tuple element value");
                Box::pin(read_value(store, r, resources, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push(v);
            }
            *val = Val::Tuple(vs);
            Ok(())
        }
        Type::Variant(ty) => {
            let discriminant = r.read_u32_leb128().await?;
            let discriminant = discriminant
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            let Case { name, ty } = ty.cases().nth(discriminant).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown variant discriminant `{discriminant}`"),
                )
            })?;
            let name = name.to_string();
            if let Some(ty) = ty {
                let mut v = Val::Bool(false);
                trace!(variant = name, "reading nested variant value");
                Box::pin(read_value(store, r, resources, &mut v, &ty, path)).await?;
                *val = Val::Variant(name, Some(Box::new(v)));
            } else {
                *val = Val::Variant(name, None);
            }
            Ok(())
        }
        Type::Enum(ty) => {
            let discriminant = r.read_u32_leb128().await?;
            let discriminant = discriminant
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            let name = ty.names().nth(discriminant).ok_or_else(|| {
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
                trace!("reading nested `option::some` value");
                Box::pin(read_value(store, r, resources, &mut v, &ty.ty(), path)).await?;
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
                    trace!("reading nested `result::ok` value");
                    Box::pin(read_value(store, r, resources, &mut v, &ty, path)).await?;
                    *val = Val::Result(Ok(Some(Box::new(v))));
                } else {
                    *val = Val::Result(Ok(None));
                }
            } else if let Some(ty) = ty.err() {
                let mut v = Val::Bool(false);
                trace!("reading nested `result::err` value");
                Box::pin(read_value(store, r, resources, &mut v, &ty, path)).await?;
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
                bits @ 129.. => {
                    let mut cap = bits / 8;
                    if bits % 8 != 0 {
                        cap = cap.saturating_add(1);
                    }
                    let mut buf = vec![0; cap];
                    r.read_exact(&mut buf).await?;
                    let mut vs = Vec::with_capacity(
                        buf.iter()
                            .map(|b| b.count_ones())
                            .sum::<u32>()
                            .try_into()
                            .unwrap_or(usize::MAX),
                    );
                    for (i, name) in names.enumerate() {
                        if buf[i / 8] & (1 << (i % 8)) != 0 {
                            vs.push(name.to_string());
                        }
                    }
                    *val = Val::Flags(vs);
                    return Ok(());
                }
            };
            let mut vs = Vec::with_capacity(flags.count_ones().try_into().unwrap_or(usize::MAX));
            for (i, name) in zip(0.., names) {
                if flags & (1 << i) != 0 {
                    vs.push(name.to_string());
                }
            }
            *val = Val::Flags(vs);
            Ok(())
        }
        Type::Own(ty) | Type::Borrow(ty) => {
            if *ty == ResourceType::host::<DynInputStream>() {
                let mut store = store.as_context_mut();
                let r = r.index(path).map_err(std::io::Error::other)?;
                // TODO: Implement a custom reader, this approach ignores the stream end (`\0`),
                // which will could potentially break/hang with some transports
                let res = store
                    .data_mut()
                    .table()
                    .push(Box::new(AsyncReadStream::new(
                        FramedRead::new(r, ListDecoderU8::default())
                            .into_async_read()
                            .compat(),
                    )))
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
                let v = res
                    .try_into_resource_any(store)
                    .map_err(std::io::Error::other)?;
                *val = Val::Resource(v);
                Ok(())
            } else if resources.contains(ty) {
                let mut store = store.as_context_mut();
                let mut id = uuid::Bytes::default();
                debug_assert_eq!(id.len(), 16);
                let n = r.read_u8_leb128().await?;
                if usize::from(n) != id.len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "invalid guest resource handle length {n}, expected {}",
                            id.len()
                        ),
                    ));
                }
                let n = r.read_exact(&mut id).await?;
                if n != id.len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "invalid amount of guest resource handle bytes read {n}, expected {}",
                            id.len()
                        ),
                    ));
                }

                let id = Uuid::from_bytes_le(id);
                trace!(?id, "lookup shared resource");
                let resource = store
                    .data_mut()
                    .shared_resources()
                    .0
                    .get(&id)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
                *val = Val::Resource(*resource);
                Ok(())
            } else {
                let mut store = store.as_context_mut();
                let n = r.read_u32_leb128().await?;
                let n = usize::try_from(n)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                let mut buf = Vec::with_capacity(n);
                r.read_to_end(&mut buf).await?;
                let table = store.data_mut().table();
                let resource = table
                    .push(RemoteResource(buf.into()))
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
                let resource = resource
                    .try_into_resource_any(store)
                    .map_err(std::io::Error::other)?;
                *val = Val::Resource(resource);
                Ok(())
            }
        }
    }
}
