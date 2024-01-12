use core::cmp::Reverse;

use core::iter::{zip, Sum};

use core::ops::{Add, Deref};
use core::str;

use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::sync::Arc;
use std::vec;

use anyhow::{anyhow, bail, ensure, Context};
use async_nats::subject::ToSubject as _;
use async_recursion::async_recursion;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use clap::Parser;

use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
use indexmap::IndexMap;
use tokio::{fs, spawn};
use tracing::{debug, error, instrument, trace, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmtime::component::{
    Component, Instance, InstancePre, Linker, LinkerInstance, Resource, ResourceAny,
    ResourceImportIndex, ResourceTable, ResourceType, Val,
};
use wasmtime::{AsContextMut, Engine};
use wasmtime_environ::component::RuntimeImportIndex;
use wasmtime_wasi::preview2::bindings::io::poll::{Host as _, HostPollable};

use wasmtime_wasi::preview2::{
    self, command, subscribe, HostInputStream, Subscribe, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};
use wit_parser::{TypeDef, TypeDefKind, TypeOwner, World, WorldItem, WorldKey};
use wrpc::nats::{connect, listen, Publisher};

const PROTOCOL: &str = "wrpc.0.0.1";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS address to use
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: String,

    /// Prefix to listen on
    #[arg(short, long, default_value = "")]
    prefix: String,

    /// Path or URL to Wasm reactor component
    workload: String,
}

pub enum Workload {
    Url(Url),
    Binary(Vec<u8>),
}

struct WasiIoResources {
    pollable: RuntimeImportIndex,
}

struct WasiResources {
    io: WasiIoResources,
}

impl WasiResources {
    pub fn lookup<T>(instance_pre: &InstancePre<T>) -> anyhow::Result<Self> {
        let wasi_io_pollable_idx = instance_pre
            .path_import_index("wasi:io/poll@0.2.0-rc-2023-11-10", &["pollable"])
            .context("failed to lookup pollable resource import index")?;
        Ok(Self {
            io: WasiIoResources {
                pollable: wasi_io_pollable_idx,
            },
        })
    }
}

struct Ctx {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    nats: Arc<async_nats::Client>,
    guest_resources: Arc<HashMap<GuestResourceType, ResourceImportIndex>>,
    //wasi_resources: Arc<WasiResources>,
    instance_pre: InstancePre<Self>,
    instance: Option<Instance>,
}

impl WasiView for Ctx {
    fn table(&self) -> &ResourceTable {
        &self.table
    }

    fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

struct RemotePollable {
    nats: Arc<async_nats::Client>,
    subject: async_nats::Subject,
    // TODO: Interface/protocol
}

#[async_trait]
impl Subscribe for RemotePollable {
    async fn ready(&mut self) -> wasmtime::Result<()> {
        let wrpc::nats::Response { payload, .. } =
            connect(&self.nats, self.subject.clone(), Bytes::new(), None)
                .await
                .context("failed to connect to peer")?;
        ensure!(payload.is_empty());
        Ok(())
    }
}

#[instrument(level = "trace", skip(store, root, child, payload))]
async fn read_values<'a>(
    mut store: impl AsContextMut<Data = Ctx> + Send,
    root: &mut async_nats::Subscriber,
    child: &mut async_nats::Subscriber,
    payload: impl Buf + Send + 'static,
    vals: &mut [Val],
    tys: &[ComponentType],
) -> anyhow::Result<Box<dyn Buf + Send>> {
    let mut payload: Box<dyn Buf + Send> = Box::new(payload);
    for (val, ty) in zip(vals, tys) {
        let (v, p) = read_value(&mut store, root, child, payload, ty)
            .await
            .context("failed to read result value")?;
        *val = v;
        payload = p;
    }
    Ok(payload)
}

#[instrument(level = "trace", skip(store, root, _child, payload))]
async fn read_value(
    mut store: impl AsContextMut<Data = Ctx> + Send,
    root: &mut async_nats::Subscriber,
    _child: &mut async_nats::Subscriber,
    payload: impl Buf + Send + 'static,
    ty: &ComponentType,
) -> anyhow::Result<(Val, Box<dyn Buf + Send>)> {
    let mut payload: Box<dyn Buf + Send> = Box::new(payload);
    let mut buf = vec![];
    loop {
        match ty {
            ComponentType::Bool => {
                if payload.remaining() >= 1 {
                    return Ok((Val::Bool(payload.get_u8() == 1), payload));
                }
            }
            ComponentType::U8 => {
                if payload.remaining() >= 1 {
                    return Ok((Val::U8(payload.get_u8()), payload));
                }
            }
            ComponentType::U16 => {
                if payload.remaining() >= 2 {
                    return Ok((Val::U16(payload.get_u16_le()), payload));
                }
            }
            ComponentType::U32 => {
                if payload.remaining() >= 4 {
                    return Ok((Val::U32(payload.get_u32_le()), payload));
                }
            }
            ComponentType::U64 => {
                if payload.remaining() >= 8 {
                    return Ok((Val::U64(payload.get_u64_le()), payload));
                }
            }
            ComponentType::S8 => {
                if payload.remaining() >= 1 {
                    return Ok((Val::S8(payload.get_i8()), payload));
                }
            }
            ComponentType::S16 => {
                if payload.remaining() >= 2 {
                    return Ok((Val::S16(payload.get_i16_le()), payload));
                }
            }
            ComponentType::S32 => {
                if payload.remaining() >= 4 {
                    return Ok((Val::S32(payload.get_i32_le()), payload));
                }
            }
            ComponentType::S64 => {
                if payload.remaining() >= 8 {
                    return Ok((Val::S64(payload.get_i64_le()), payload));
                }
            }
            ComponentType::Float32 => {
                if payload.remaining() >= 4 {
                    return Ok((Val::Float32(payload.get_f32_le()), payload));
                }
            }
            ComponentType::Float64 => {
                if payload.remaining() >= 8 {
                    return Ok((Val::Float64(payload.get_f64_le()), payload));
                }
            }
            ComponentType::Char => {
                if payload.remaining() >= 4 {
                    let v = payload
                        .get_u32_le()
                        .try_into()
                        .context("char is not valid")?;
                    return Ok((Val::Char(v), payload));
                }
            }
            ComponentType::String => {
                let mut r = payload.reader();
                r.read_until(0, &mut buf)
                    .context("failed to read from buffer")?;
                match buf.pop() {
                    Some(0) => {
                        let v = String::from_utf8(buf).context("string is not valid UTF-8")?;
                        return Ok((Val::String(v.into()), r.into_inner()));
                    }
                    Some(c) => {
                        buf.push(c);
                    }
                    _ => {}
                }
                payload = r.into_inner();
            }
            ComponentType::List(_)
            | ComponentType::Record(_)
            | ComponentType::Tuple(_)
            | ComponentType::Variant(_)
            | ComponentType::Enum(_)
            | ComponentType::Option(_)
            | ComponentType::Result { .. }
            | ComponentType::Flags(_) => {
                bail!("blocked on https://github.com/bytecodealliance/wasmtime/issues/7726")
            }
            ComponentType::Resource(ty) => {
                let mut store = store.as_context_mut();
                match ty {
                    ComponentResourceType::Pollable => {
                        if payload.remaining() >= 1 {
                            match payload.get_u8() {
                                0 => {
                                    //let pollable = table
                                    //    .push(RemotePollable {
                                    //        nats: Arc::clone(nats),
                                    //        subject: v.to_subject(),
                                    //    })
                                    //    .context("failed to push pollable to table")?;
                                    // let _res = subscribe(table, pollable)
                                    // .context("failed to subscribe to pollable")?;
                                    bail!("pending pollables not supported yet")
                                }
                                1 => {
                                    struct Ready;

                                    #[async_trait]
                                    impl Subscribe for Ready {
                                        async fn ready(&mut self) -> wasmtime::Result<()> {
                                            Ok(())
                                        }
                                    }

                                    let pollable = store
                                        .data_mut()
                                        .table_mut()
                                        .push(Ready)
                                        .context("failed to push resource to table")?;
                                    let _pollable =
                                        subscribe(store.data_mut().table_mut(), pollable)
                                            .context("failed to subscribe")?;
                                    let _instance_pre = store.data().instance_pre.clone();
                                    //let idx = store.data().wasi_resources.io.pollable;
                                    //let pollable = pollable
                                    //    .try_into_resource_any(&mut store, &instance_pre, idx)
                                    //    .context("failed to convert pollable to `ResourceAny`")?;
                                    bail!("ready pollables not supported yet")
                                }
                                _ => bail!("invalid `pollable` value"),
                            }
                            //return Ok(payload);
                        }
                    }
                    ComponentResourceType::InputStream => todo!(),
                    ComponentResourceType::OutputStream => todo!(),
                    ComponentResourceType::Guest(ty) => {
                        trace!(?ty, "decode guest type");
                        let mut r = payload.reader();
                        r.read_until(0, &mut buf)
                            .context("failed to read from buffer")?;
                        match buf.pop() {
                            Some(0) => {
                                let subject =
                                    String::from_utf8(buf).context("string is not valid UTF-8")?;
                                let subject = store
                                    .data_mut()
                                    .table_mut()
                                    .push(subject.to_subject())
                                    .context("failed to push guest resource to table")?;
                                let instance_pre = store.data().instance_pre.clone();
                                let idx = *store
                                    .data()
                                    .guest_resources
                                    .get(ty)
                                    .context("failed to lookup guest resource type index")?;
                                let idx = instance_pre
                                    .resource_import_index(idx)
                                    .context("failed to lookup resource runtime import index")?;
                                let subject = subject
                                    .try_into_resource_any(&mut store, &instance_pre, idx)
                                    .context("failed to convert resource to `ResourceAny`")?;
                                return Ok((Val::Resource(subject), r.into_inner()));
                            }
                            Some(c) => {
                                buf.push(c);
                            }
                            _ => {}
                        }
                        payload = r.into_inner();
                    }
                }
            }
        }
        // TODO: timeout
        trace!("await root message");
        let msg = root.next().await.context("failed to receive message")?;
        trace!(?msg, "root message received");
        payload = Box::new(payload.chain(msg.payload))
    }
}

#[derive(Debug)]
struct GuestLocalPollable {
    guest_resource: ResourceAny,
    pollable: Resource<preview2::Pollable>,
}

#[derive(Debug)]
struct GuestResource {
    resource: ResourceAny,
    listener: wrpc::nats::Listener,
    subject: String,
    ty: GuestResourceType,
}

#[derive(Debug)]
enum GuestAsyncValue {
    Pollable(GuestLocalPollable),
    Resource(GuestResource),
}

#[derive(Debug)]
struct GuestResourceCallPollable {
    listener: wrpc::nats::Listener,
    request: Option<anyhow::Result<wrpc::nats::Request>>,
}

#[async_trait]
impl Subscribe for GuestResourceCallPollable {
    async fn ready(&mut self) -> wasmtime::Result<()> {
        if self.request.is_some() {
            Ok(())
        } else {
            self.request = self.listener.next().await;
            Ok(())
        }
    }
}

#[derive(Debug)]
struct GuestResourcePollable {
    pollable: Resource<preview2::Pollable>,
    inner: Resource<GuestResourceCallPollable>,
    guest_resource: ResourceAny,
    subject: String,
    ty: GuestResourceType,
}

#[derive(Debug)]
enum GuestPollable {
    Local {
        tx: Publisher,
        pollable: GuestLocalPollable,
    },
    Resource(GuestResourcePollable),
}

impl GuestPollable {
    fn pollable(&self) -> &Resource<preview2::Pollable> {
        match self {
            Self::Local {
                pollable: GuestLocalPollable { pollable, .. },
                ..
            } => pollable,
            Self::Resource(GuestResourcePollable { pollable, .. }) => pollable,
        }
    }
}

#[derive(Debug, Default)]
struct GuestAsyncValues(Vec<(VecDeque<u32>, GuestAsyncValue)>);

impl Deref for GuestAsyncValues {
    type Target = Vec<(VecDeque<u32>, GuestAsyncValue)>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for GuestAsyncValues {
    type Item = (VecDeque<u32>, GuestAsyncValue);
    type IntoIter = vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl From<GuestAsyncValue> for GuestAsyncValues {
    fn from(value: GuestAsyncValue) -> Self {
        Self(vec![(VecDeque::default(), value)])
    }
}

impl Extend<(VecDeque<u32>, GuestAsyncValue)> for GuestAsyncValues {
    fn extend<T: IntoIterator<Item = (VecDeque<u32>, GuestAsyncValue)>>(&mut self, iter: T) {
        self.0.extend(iter)
    }
}

impl Extend<Self> for GuestAsyncValues {
    fn extend<T: IntoIterator<Item = Self>>(&mut self, iter: T) {
        for Self(vs) in iter {
            self.extend(vs)
        }
    }
}

impl Add for GuestAsyncValues {
    type Output = Self;

    fn add(mut self, Self(vs): Self) -> Self::Output {
        self.extend(vs);
        self
    }
}

impl Sum for GuestAsyncValues {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.reduce(Add::add).unwrap_or_default()
    }
}

impl FromIterator<Self> for GuestAsyncValues {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        iter.into_iter().sum()
    }
}

impl GuestAsyncValues {
    fn nest_at(&mut self, idx: u32) {
        for (path, _) in self.0.iter_mut() {
            path.push_front(idx)
        }
    }

    fn into_pollables(
        self,
        mut store: impl AsContextMut<Data = Ctx> + Send,
        tx: &wrpc::nats::Publisher,
    ) -> anyhow::Result<Vec<GuestPollable>> {
        let mut store = store.as_context_mut();
        let mut pollables = Vec::with_capacity(self.0.len());
        for (path, fut) in self {
            match fut {
                GuestAsyncValue::Pollable(pollable) => {
                    let mut iter = path.iter().map(ToString::to_string);
                    let seps = path.len().checked_sub(1).context("path cannot be empty")?;
                    let path_len = iter
                        .by_ref()
                        .map(|idx| idx.len())
                        .sum::<usize>()
                        .checked_add(seps)
                        .context("invalid path length")?;
                    let path = iter.fold(String::with_capacity(path_len), |mut path, idx| {
                        path.push_str(&idx);
                        path
                    });
                    pollables.push(GuestPollable::Local {
                        tx: tx.child(&path),
                        pollable,
                    });
                }
                GuestAsyncValue::Resource(GuestResource {
                    resource,
                    listener,
                    subject,
                    ty,
                }) => {
                    let inner = GuestResourceCallPollable {
                        request: None,
                        listener,
                    };
                    let inner =
                        store.data_mut().table_mut().push(inner).context(
                            "failed to push guest resource call stream resource to table",
                        )?;
                    let pollable = subscribe(
                        store.data_mut().table_mut(),
                        Resource::<GuestResourceCallPollable>::new_borrow(inner.rep()),
                    )
                    .context("failed to subscribe to guest resource call stream")?;
                    pollables.push(GuestPollable::Resource(GuestResourcePollable {
                        pollable,
                        inner,
                        guest_resource: resource,
                        subject,
                        ty,
                    }))
                }
            }
        }
        Ok(pollables)
    }
}

#[instrument(level = "trace", skip_all)]
#[async_recursion]
async fn write_value_iter<'a>(
    mut store: impl AsContextMut<Data = Ctx> + Send + 'async_recursion,
    payload: &mut BytesMut,
    iter: impl Iterator<Item = ComponentValue<'a>> + Send + 'async_recursion,
) -> anyhow::Result<GuestAsyncValues> {
    let mut futs = GuestAsyncValues::default();
    for (i, v) in zip(0.., iter) {
        let mut vs = write_value(&mut store, payload, v).await?;
        vs.nest_at(i);
        futs = futs.add(vs);
    }
    Ok(futs)
}

#[instrument(level = "trace", skip_all)]
#[async_recursion]
async fn write_value<'a: 'async_recursion>(
    mut store: impl AsContextMut<Data = Ctx> + Send + 'async_recursion,
    payload: &mut BytesMut,
    v: ComponentValue<'a>,
) -> anyhow::Result<GuestAsyncValues> {
    let mut store = store.as_context_mut();
    match (v.value, v.ty) {
        (Val::Bool(false), ComponentType::Bool) => {
            payload.put_u8(0);
            Ok(GuestAsyncValues::default())
        }
        (Val::Bool(true), ComponentType::Bool) => {
            payload.put_u8(1);
            Ok(GuestAsyncValues::default())
        }
        (Val::S8(v), ComponentType::S8) => {
            payload.put_i8(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::U8(v), ComponentType::U8) => {
            payload.put_u8(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::S16(v), ComponentType::S16) => {
            payload.put_i16_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::U16(v), ComponentType::U16) => {
            payload.put_u16_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::S32(v), ComponentType::S32) => {
            payload.put_i32_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::U32(v), ComponentType::U32) => {
            payload.put_u32_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::S64(v), ComponentType::S64) => {
            payload.put_i64_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::U64(v), ComponentType::U64) => {
            payload.put_u64_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::Float32(v), ComponentType::Float32) => {
            payload.put_f32_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::Float64(v), ComponentType::Float64) => {
            payload.put_f64_le(*v);
            Ok(GuestAsyncValues::default())
        }
        (Val::Char(v), ComponentType::Char) => {
            payload.put_u32_le(u32::from(*v));
            Ok(GuestAsyncValues::default())
        }
        (Val::String(v), ComponentType::String) => {
            payload.put_slice(v.as_bytes());
            payload.put_u8(0);
            Ok(GuestAsyncValues::default())
        }
        (Val::List(vs), ComponentType::List(ty)) => {
            let vs = vs.deref();
            let len: u32 = vs
                .len()
                .try_into()
                .context("list length does not fit in u32")?;
            payload.put_u32_le(len);
            write_value_iter(
                store,
                payload,
                vs.iter().map(|value| ComponentValue { value, ty }),
            )
            .await
        }
        (Val::Record(vs), ComponentType::Record(tys)) => {
            write_value_iter(
                store,
                payload,
                vs.fields()
                    .zip(tys)
                    .map(|((_, value), ty)| ComponentValue { value, ty }),
            )
            .await
        }
        (Val::Tuple(vs), ComponentType::Tuple(tys)) => {
            write_value_iter(
                store,
                payload,
                vs.values()
                    .iter()
                    .zip(tys)
                    .map(|(value, ty)| ComponentValue { value, ty }),
            )
            .await
        }
        (Val::Variant(_v), ComponentType::Variant(..)) => {
            bail!("variants not supported yet")
        }
        (Val::Enum(_v), ComponentType::Enum(..)) => {
            bail!("enums not supported yet")
        }
        (Val::Option(v), ComponentType::Option(ty)) => {
            if let Some(value) = v.value() {
                payload.put_u8(1);
                write_value(&mut store, payload, ComponentValue { value, ty }).await
            } else {
                payload.put_u8(0);
                Ok(GuestAsyncValues::default())
            }
        }
        (Val::Result(v), ty) => match (v.value(), ty) {
            (Ok(None), ComponentType::Result { ok: None, .. }) => {
                payload.put_u8(1);
                Ok(GuestAsyncValues::default())
            }
            (Ok(Some(value)), ComponentType::Result { ok: Some(ty), .. }) => {
                payload.put_u8(1);
                write_value(&mut store, payload, ComponentValue { value, ty }).await
            }
            (Err(None), ComponentType::Result { err: None, .. }) => {
                payload.put_u8(0);
                Ok(GuestAsyncValues::default())
            }
            (Err(Some(value)), ComponentType::Result { err: Some(ty), .. }) => {
                payload.put_u8(0);
                write_value(&mut store, payload, ComponentValue { value, ty }).await
            }
            _ => bail!("value type mismatch"),
        },
        (Val::Flags(_v), ComponentType::Flags(..)) => {
            bail!("flags not supported yet")
        }
        (Val::Resource(resource), ComponentType::Resource(ComponentResourceType::Pollable)) => {
            let pollable: Resource<preview2::Pollable> = resource
                .try_into_resource(&mut store)
                .context("failed to obtain pollable")?;
            let ready = store
                .data_mut()
                .ready(Resource::new_borrow(pollable.rep()))
                .await
                .context("failed to check pollable readiness")?;
            if ready {
                //resource
                //    .resource_drop_async(&mut store)
                //    .await
                //    .context("failed to drop resource")?;
                store
                    .data_mut()
                    .drop(pollable)
                    .context("failed to drop pollable")?;
                payload.put_u8(1);
                Ok(GuestAsyncValues::default())
            } else {
                payload.put_u8(0);
                Ok(GuestAsyncValues::from(GuestAsyncValue::Pollable(
                    GuestLocalPollable {
                        guest_resource: *resource,
                        pollable,
                    },
                )))
            }
        }
        (Val::Resource(resource), ComponentType::Resource(ComponentResourceType::InputStream)) => {
            let _stream: Resource<preview2::pipe::AsyncReadStream> = resource
                .try_into_resource(&mut store)
                .context("failed to obtain read stream")?;
            //if stream.owned() {
            //    let stream = store
            //        .data_mut()
            //        .table_mut()
            //        .delete(res)
            //        .context("failed to delete resource")?;
            //} else {
            //    let stream = store
            //        .data()
            //        .table()
            //        .get(&res)
            //        .context("failed to get resource")?;
            //};
            // if ready and closed, write [1, bytes]
            // if not closed, [0]
            bail!("streams not supported yet")
        }
        (Val::Resource(resource), ComponentType::Resource(ComponentResourceType::Guest(ty))) => {
            if resource.ty() == ResourceType::host::<async_nats::Subject>() {
                let subject: Resource<async_nats::Subject> = resource
                    .try_into_resource(&mut store)
                    .context("failed to obtain custom resource")?;
                if subject.owned() {
                    let subject = store
                        .data_mut()
                        .table_mut()
                        .delete(subject)
                        .context("failed to delete subject resource")?;
                    payload.put(subject.as_bytes());
                } else {
                    let subject = store
                        .data()
                        .table()
                        .get(&subject)
                        .context("failed to get resource")?;
                    payload.put(subject.as_bytes());
                };
                payload.put_u8(0);
                Ok(GuestAsyncValues::default())
            } else {
                let nats = &store.data().nats;
                let subject = nats.new_inbox();
                payload.put_slice(subject.as_bytes());
                payload.put_u8(0);
                let listener = listen(nats, format!("{subject}.>"))
                    .await
                    .context("failed to listen on inbox")?;
                Ok(GuestAsyncValues::from(GuestAsyncValue::Resource(
                    GuestResource {
                        listener,
                        subject,
                        resource: *resource,
                        ty: ty.clone(),
                    },
                )))
            }
        }
        _ => bail!("value type mismatch"),
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct GuestResourceType {
    instance: String,
    name: String,
}

#[derive(Clone, Debug)]
enum ComponentResourceType {
    Pollable,
    InputStream,
    OutputStream,
    Guest(GuestResourceType),
}

#[derive(Clone, Debug)]
enum ComponentType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    Float32,
    Float64,
    Char,
    String,
    List(Box<ComponentType>),
    Record(Vec<ComponentType>),
    Tuple(Vec<ComponentType>),
    Variant(Vec<Option<ComponentType>>),
    Enum(u32),
    Option(Box<ComponentType>),
    Result {
        ok: Option<Box<ComponentType>>,
        err: Option<Box<ComponentType>>,
    },
    Flags(u32),
    Resource(ComponentResourceType),
}

#[derive(Debug)]
enum ComponentResourceValue {
    Pollable(Resource<preview2::Pollable>),
    InputStream(Resource<preview2::pipe::AsyncReadStream>),
    OutputStream(Resource<preview2::pipe::AsyncWriteStream>),
    Guest(ResourceAny),
}

#[derive(Debug, Clone)]
struct ComponentValue<'a> {
    value: &'a Val,
    ty: &'a ComponentType,
}

impl<'a> From<(&'a Val, &'a ComponentType)> for ComponentValue<'a> {
    fn from((value, ty): (&'a Val, &'a ComponentType)) -> Self {
        Self { value, ty }
    }
}

#[derive(Debug)]
struct ComponentValueMut<'a> {
    value: &'a mut Val,
    ty: &'a ComponentType,
}

impl<'a> From<(&'a mut Val, &'a ComponentType)> for ComponentValueMut<'a> {
    fn from((value, ty): (&'a mut Val, &'a ComponentType)) -> Self {
        Self { value, ty }
    }
}

#[instrument(level = "trace", skip(resolve), ret)]
fn resolve_type_def(resolve: &wit_parser::Resolve, ty: &TypeDef) -> anyhow::Result<ComponentType> {
    match ty {
        TypeDef {
            kind: TypeDefKind::Record(wit_parser::Record { fields: _, .. }),
            ..
        } => todo!(),
        TypeDef {
            name,
            kind: TypeDefKind::Resource,
            owner,
            ..
        } => {
            let name = name.as_ref().context("resource is missing a name")?;
            match owner {
                TypeOwner::Interface(interface) => {
                    let interface = resolve
                        .interfaces
                        .get(*interface)
                        .context("resource belongs to a non-existent interface")?;
                    let interface_name = interface
                        .name
                        .as_ref()
                        .context("interface is missing a name")?;
                    let pkg = interface
                        .package
                        .context("interface is missing a package")?;
                    let root = resolve.id_of_name(pkg, interface_name);
                    match (root.as_str(), name.as_str()) {
                        ("wasi:io/poll@0.2.0-rc-2023-11-10", "pollable") => {
                            Ok(ComponentType::Resource(ComponentResourceType::Pollable))
                        }
                        _ => Ok(ComponentType::Resource(ComponentResourceType::Guest(
                            GuestResourceType {
                                instance: root,
                                name: name.to_string(),
                            },
                        ))),
                    }
                }
                _ => bail!("only resources owned by an interface are currently supported"),
            }
        }
        TypeDef {
            kind: TypeDefKind::Handle(wit_parser::Handle::Own(ty) | wit_parser::Handle::Borrow(ty)),
            ..
        } => {
            let ty = resolve
                .types
                .get(*ty)
                .context("unknown handle inner type")?;
            resolve_type_def(resolve, ty)
        }
        TypeDef {
            kind: TypeDefKind::Enum(wit_parser::Enum { cases: _, .. }),
            ..
        } => todo!(),
        TypeDef {
            kind: TypeDefKind::Option(ty),
            ..
        } => {
            let ty = resolve_type(resolve, ty).context("failed to resolve inner option type")?;
            Ok(ComponentType::Option(Box::new(ty)))
        }
        TypeDef {
            kind: TypeDefKind::Type(ty),
            ..
        } => resolve_type(resolve, ty).context("failed to resolve inner handle type"),
        TypeDef { kind, .. } => bail!("{} types not supported yet", kind.as_str()),
    }
}

#[instrument(level = "trace", skip(resolve), ret)]
fn resolve_type(
    resolve: &wit_parser::Resolve,
    ty: &wit_parser::Type,
) -> anyhow::Result<ComponentType> {
    match ty {
        wit_parser::Type::Bool => Ok(ComponentType::Bool),
        wit_parser::Type::U8 => Ok(ComponentType::U8),
        wit_parser::Type::U16 => Ok(ComponentType::U16),
        wit_parser::Type::U32 => Ok(ComponentType::U32),
        wit_parser::Type::U64 => Ok(ComponentType::U64),
        wit_parser::Type::S8 => Ok(ComponentType::S8),
        wit_parser::Type::S16 => Ok(ComponentType::S16),
        wit_parser::Type::S32 => Ok(ComponentType::S32),
        wit_parser::Type::S64 => Ok(ComponentType::S64),
        wit_parser::Type::Float32 => Ok(ComponentType::Float32),
        wit_parser::Type::Float64 => Ok(ComponentType::Float64),
        wit_parser::Type::Char => Ok(ComponentType::Char),
        wit_parser::Type::String => Ok(ComponentType::String),
        wit_parser::Type::Id(ty) => {
            let ty = resolve.types.get(*ty).context("unknown type")?;
            resolve_type_def(resolve, ty)
        }
    }
}

enum HostType {
    Guest(ComponentType),
    Future(Box<HostType>),
    Stream(Box<HostType>),
}

#[instrument(level = "trace", skip(component, linker, resolve, exports), ret)]
fn polyfill_function(
    component: &Component,
    linker: &mut LinkerInstance<Ctx>,
    resolve: &wit_parser::Resolve,
    exports: &Arc<HashMap<String, HashMap<String, FunctionExport>>>,
    instance_name: &str,
    func_name: &str,
    wit_parser::Function {
        params: params_ty,
        results: results_ty,
        kind,
        ..
    }: &wit_parser::Function,
) -> anyhow::Result<()> {
    let params_ty = params_ty
        .iter()
        .map(|(_, ty)| resolve_type(resolve, ty))
        .collect::<anyhow::Result<Vec<_>>>()
        .context("failed to resolve parameter types")?;
    let params_ty = Arc::new(params_ty);
    let results_ty = results_ty
        .iter_types()
        .map(|ty| resolve_type(resolve, ty))
        .collect::<anyhow::Result<Vec<_>>>()
        .context("failed to resolve result types")?;
    let results_ty = Arc::new(results_ty);
    match kind {
        wit_parser::FunctionKind::Freestanding => {
            let instance_name = Arc::new(instance_name.to_string());
            let func_name = Arc::new(func_name.to_string());
            let exports = Arc::clone(exports);
            linker
                .func_new_async(
                    component,
                    &Arc::clone(&func_name),
                    move |mut store, params, results| {
                        let instance_name = Arc::clone(&instance_name);
                        let func_name = Arc::clone(&func_name);
                        let params_ty = Arc::clone(&params_ty);
                        let results_ty = Arc::clone(&results_ty);
                        let exports = Arc::clone(&exports);
                        Box::new(async move {
                            let nats = Arc::clone(&store.data().nats);

                            let mut payload = BytesMut::new();
                            let futs = write_value_iter(
                                &mut store,
                                &mut payload,
                                params.iter().zip(params_ty.iter()).map(Into::into),
                            )
                            .await
                            .context("failed to encode parameters")?;

                            // TODO: Protocol/interface negotiation in headers

                            trace!(?instance_name, ?func_name, "call freestanding function");
                            let wrpc::nats::Response { payload, conn, .. } = connect(
                                &nats,
                                format!("{PROTOCOL}.{instance_name}/{func_name}"),
                                payload.freeze(),
                                None,
                            )
                            .await
                            .context("failed to connect to peer")?;
                            trace!(
                                ?instance_name,
                                ?func_name,
                                ?payload,
                                "received freestanding function response"
                            );
                            let (tx, mut root, mut child) = conn.split();
                            if let Some(tx) = tx {
                                let pollables = futs
                                    .into_pollables(&mut store, &tx)
                                    .context("failed to construct pollables")?;
                                tx.sized().flush(&nats).await.context("failed to flush")?;
                                let instance = store
                                    .data()
                                    .instance
                                    .clone()
                                    .context("instance missing from store")?;
                                handle_pollables(&mut store, &instance, &nats, &exports, pollables)
                                    .await
                                    .context("failed to handle pollables")?;
                            }
                            read_values(
                                &mut store,
                                &mut root,
                                &mut child,
                                payload,
                                results,
                                &results_ty,
                            )
                            .await
                            .context("failed to decode results")?;
                            trace!(
                                ?instance_name,
                                ?func_name,
                                ?results,
                                "decoded freestanding function response"
                            );
                            Ok(())
                        })
                    },
                )
                .context("failed to polyfill freestanding function")
        }
        wit_parser::FunctionKind::Constructor(_) => {
            bail!("constructors not supported yet")
        }
        wit_parser::FunctionKind::Static(_) => {
            bail!("static functions not supported yet")
        }
        wit_parser::FunctionKind::Method(_) => {
            let (_, name) = func_name
                .split_once('.')
                .context("method name is not valid")?;
            let name = Arc::new(name.to_string());
            let exports = Arc::clone(exports);
            linker
                .func_new_async(component, func_name, move |mut store, params, results| {
                    let name = Arc::clone(&name);
                    let params_ty = Arc::clone(&params_ty);
                    let results_ty = Arc::clone(&results_ty);
                    let exports = Arc::clone(&exports);
                    Box::new(async move {
                        let (this, params) = params
                            .split_first()
                            .context("method receiver parameter missing")?;
                        let Val::Resource(this) = this else {
                            bail!("method receiver is not a resource");
                        };
                        let res: Resource<async_nats::Subject> = this
                            .try_into_resource(&mut store)
                            .context("failed to obtain custom resource")?;
                        let subject = if this.owned() {
                            let subject = store
                                .data_mut()
                                .table_mut()
                                .delete(res)
                                .context("failed to delete resource")?;
                            format!("{subject}.{name}")
                        } else {
                            let subject = store
                                .data()
                                .table()
                                .get(&res)
                                .context("failed to get resource")?;
                            format!("{subject}.{name}")
                        };
                        this.resource_drop_async(&mut store)
                            .await
                            .context("failed to drop subject resource")?;
                        let nats = Arc::clone(&store.data().nats);

                        let mut payload = BytesMut::new();
                        let futs = write_value_iter(
                            &mut store,
                            &mut payload,
                            params.iter().zip(params_ty.iter().skip(1)).map(Into::into),
                        )
                        .await
                        .context("failed to encode parameters")?;

                        // TODO: Protocol/interface negotiation in headers

                        let wrpc::nats::Response { payload, conn, .. } =
                            connect(&nats, subject, payload.freeze(), None)
                                .await
                                .context("failed to connect to peer")?;
                        let (tx, mut root, mut child) = conn.split();
                        if let Some(tx) = tx {
                            let pollables = futs
                                .into_pollables(&mut store, &tx)
                                .context("failed to construct pollables")?;
                            tx.sized().flush(&nats).await.context("failed to flush")?;
                            let instance = store
                                .data()
                                .instance
                                .clone()
                                .context("instance missing from store")?;
                            handle_pollables(&mut store, &instance, &nats, &exports, pollables)
                                .await
                                .context("failed to handle pollables")?;
                        }
                        read_values(
                            &mut store,
                            &mut root,
                            &mut child,
                            payload,
                            results,
                            &results_ty,
                        )
                        .await
                        .context("failed to decode results")?;
                        Ok(())
                    })
                })
                .context("failed to polyfill method")
        }
    }
}

#[instrument(level = "trace", skip_all, ret)]
fn polyfill(
    component: &Component,
    resolve: &wit_parser::Resolve,
    exports: &Arc<HashMap<String, HashMap<String, FunctionExport>>>,
    imports: &IndexMap<WorldKey, WorldItem>,
    linker: &mut Linker<Ctx>,
) -> HashMap<GuestResourceType, ResourceImportIndex> {
    let mut paths = HashMap::new();
    for (wk, wi) in imports {
        let root = resolve.name_world_key(wk);
        match root.as_str() {
            "wasi:cli/environment@0.2.0-rc-2023-12-05"
            | "wasi:cli/exit@0.2.0-rc-2023-12-05"
            | "wasi:cli/stderr@0.2.0-rc-2023-12-05"
            | "wasi:cli/stdin@0.2.0-rc-2023-12-05"
            | "wasi:cli/stdout@0.2.0-rc-2023-12-05"
            | "wasi:cli/terminal-input@0.2.0-rc-2023-11-10"
            | "wasi:cli/terminal-output@0.2.0-rc-2023-11-10"
            | "wasi:cli/terminal-stderr@0.2.0-rc-2023-11-10"
            | "wasi:cli/terminal-stdin@0.2.0-rc-2023-11-10"
            | "wasi:cli/terminal-stdout@0.2.0-rc-2023-11-10"
            | "wasi:clocks/monotonic-clock@0.2.0-rc-2023-11-10"
            | "wasi:clocks/wall-clock@0.2.0-rc-2023-11-10"
            | "wasi:filesystem/preopens@0.2.0-rc-2023-11-10"
            | "wasi:filesystem/types@0.2.0-rc-2023-11-10"
            | "wasi:io/error@0.2.0-rc-2023-11-10"
            | "wasi:io/poll@0.2.0-rc-2023-11-10"
            | "wasi:io/streams@0.2.0-rc-2023-11-10"
            | "wasi:sockets/tcp@0.2.0-rc-2023-11-10" => continue,
            instance_name => {
                let WorldItem::Interface(interface) = wi else {
                    continue;
                };
                let mut linker = linker.root();
                let mut linker = match linker.instance(instance_name) {
                    Ok(linker) => linker,
                    Err(err) => {
                        error!(
                            ?err,
                            instance_name, "failed to instantiate interface from root"
                        );
                        continue;
                    }
                };
                let Some(wit_parser::Interface {
                    types, functions, ..
                }) = resolve.interfaces.get(*interface)
                else {
                    warn!("component imports a non-existent interface");
                    continue;
                };
                for (func_name, ty) in functions {
                    if let Err(err) = polyfill_function(
                        component,
                        &mut linker,
                        resolve,
                        exports,
                        instance_name,
                        func_name,
                        ty,
                    ) {
                        warn!(
                            ?err,
                            instance_name, func_name, "failed to polyfill function"
                        )
                    }
                }
                for (name, ty) in types {
                    let Some(ty) = resolve.types.get(*ty) else {
                        warn!("component imports a non-existent type");
                        continue;
                    };
                    error!("resolve type definition {instance_name} {name} {ty:?}");
                    match resolve_type_def(&resolve, ty) {
                        Ok(ComponentType::Resource(ComponentResourceType::Guest(ty)))
                            if ty.instance == instance_name =>
                        {
                            match linker.resource(
                                name,
                                ResourceType::host::<async_nats::Subject>(),
                                |_, _| Ok(()),
                            ) {
                                Ok(idx) => {
                                    paths.insert(ty, idx);
                                }
                                Err(err) => {
                                    error!(?err, name, "failed to polyfill resource")
                                }
                            }
                        }
                        Ok(ty) => debug!(?ty, instance_name, "avoid polyfilling type"),
                        Err(err) => warn!(?err, "failed to resolve type definition"),
                    };
                }
            }
        }
    }
    paths
}

#[instrument(level = "trace", skip(store, nats, exports, instance, pollables), ret)]
async fn handle_pollables(
    mut store: impl AsContextMut<Data = Ctx> + Send,
    instance: &Instance,
    nats: &Arc<async_nats::Client>,
    exports: &HashMap<String, HashMap<String, FunctionExport>>,
    mut pollables: Vec<GuestPollable>,
) -> anyhow::Result<()> {
    let mut store = store.as_context_mut();
    while !pollables.is_empty() {
        let mut ready = store
            .data_mut()
            .poll(
                pollables
                    .iter()
                    .map(|pollable| Resource::new_borrow(pollable.pollable().rep()))
                    .collect(),
            )
            .await
            .context("failed to await pollables")?;
        // Iterate in reverse order to not break the mapping when removing/pushing entries
        ready.sort_by_key(|idx| Reverse(*idx));
        for idx in ready {
            let idx = idx.try_into().context("invalid ready list index")?;
            match pollables.remove(idx) {
                GuestPollable::Local {
                    mut tx,
                    pollable: GuestLocalPollable { guest_resource, .. },
                } => {
                    guest_resource
                        .resource_drop_async(&mut store)
                        .await
                        .context("failed to drop guest resource")?;
                    *tx.buffer_mut() = Bytes::new();
                    tx.sized()
                        .flush(&nats)
                        .await
                        .context("failed to flush pollable")?;
                }
                GuestPollable::Resource(resource) => {
                    let GuestResourceCallPollable { request, .. } = store
                        .data_mut()
                        .table_mut()
                        .get_mut(&resource.inner)
                        .context("failed to get guest resource call stream")?;
                    let wrpc::nats::Request {
                        payload,
                        subject,
                        conn,
                        ..
                    } = request
                        .take()
                        .context("invalid poll state")?
                        .context("failed to handle resource method call request")?;
                    let name = subject
                        .strip_prefix(&resource.subject)
                        .context("failed to strip prefix")?;
                    let (_, name) = name
                        .split_once('.')
                        .context("subject missing a dot separator")?;
                    if name == "drop" {
                        error!("TODO: drop resource pollables");
                        resource
                            .guest_resource
                            .resource_drop_async(&mut store)
                            .await
                            .context("failed to drop guest resource")?;
                        continue;
                    }

                    let name = format!("{}.{name}", resource.ty.name);
                    error!(name, resource.ty.instance, ?exports, "lookup method");
                    let Some(FunctionExport::Method {
                        receiver: receiver_ty,
                        params: params_ty,
                        results: results_ty,
                    }) = exports
                        .get(&resource.ty.instance)
                        .and_then(|functions| functions.get(&name))
                    else {
                        bail!("export type is not a method")
                    };
                    ensure!(*receiver_ty == resource.ty, "method receiver type mismatch");

                    let func = {
                        let mut exports = instance.exports(&mut store);
                        let mut interface =
                            exports.instance(&resource.ty.instance).with_context(|| {
                                format!("instance of `{}` not found", resource.ty.instance)
                            })?;
                        interface
                            .func(&format!("[method]{name}"))
                            .with_context(|| format!("function `{name}` not found"))?
                    };
                    let conn = conn
                        .connect(&nats, Bytes::new(), None)
                        .await
                        .context("failed to connect to peer")?;
                    let (tx, mut root, mut child) = conn.split();
                    let mut params = vec![Val::Bool(false); params_ty.len() + 1];
                    let (receiver, vals) = params
                        .split_first_mut()
                        .expect("invalid parameter vector generated");
                    read_values(&mut store, &mut root, &mut child, payload, vals, params_ty)
                        .await
                        .context("failed to decode results")?;
                    *receiver = Val::Resource(if resource.guest_resource.owned() {
                        resource.guest_resource.borrow()
                    } else {
                        resource.guest_resource
                    });
                    let mut results = vec![Val::Bool(false); results_ty.len()];
                    func.call_async(&mut store, &params, &mut results)
                        .await
                        .context("failed to call async")?;
                    let mut payload = BytesMut::new();
                    let futs = write_value_iter(
                        &mut store,
                        &mut payload,
                        results.iter().zip(results_ty.as_ref()).map(Into::into),
                    )
                    .await
                    .context("failed to encode results")?;
                    let mut tx = tx.expect("reply missing for accepted request");
                    let resource_pollables = futs
                        .into_pollables(&mut store, &tx)
                        .context("failed to construct pollables")?;
                    *tx.buffer_mut() = payload.freeze();
                    tx.sized().flush(&nats).await.context("failed to flush")?;
                    func.post_return_async(&mut store)
                        .await
                        .context("failed to perform post-return cleanup")?;

                    // Push pollable back
                    pollables.push(GuestPollable::Resource(resource));
                    pollables.extend(resource_pollables);
                }
            }
        }
    }
    Ok(())
}

#[instrument(
    level = "trace",
    skip(nats, engine, instance_pre, guest_resources, exports, payload, conn),
    ret
)]
async fn handle_connection(
    nats: Arc<async_nats::Client>,
    engine: &Engine,
    instance_pre: &InstancePre<Ctx>,
    guest_resources: Arc<HashMap<GuestResourceType, ResourceImportIndex>>,
    exports: &HashMap<String, HashMap<String, FunctionExport>>,
    instance_name: &str,
    func_name: &str,
    params_ty: &[ComponentType],
    results_ty: &[ComponentType],
    wrpc::nats::Request { payload, conn, .. }: wrpc::nats::Request,
) -> anyhow::Result<()> {
    let table = ResourceTable::new();
    let mut wasi = WasiCtxBuilder::new();
    let wasi = if instance_name.is_empty() {
        wasi.args(&[func_name])
    } else {
        wasi.args(&[instance_name, func_name])
    }
    .inherit_stdio()
    .build();
    //let wasi_resources = WasiResources::lookup(&instance_pre)
    //    .context("failed to lookup WASI resource types indexes")?;
    let ctx = Ctx {
        wasi,
        http: WasiHttpCtx,
        table,
        nats: Arc::clone(&nats),
        guest_resources,
        //wasi_resources: Arc::new(wasi_resources),
        instance_pre: instance_pre.clone(),
        instance: None,
    };
    let mut store = wasmtime::Store::new(engine, ctx);
    let instance = instance_pre
        .instantiate_async(&mut store)
        .await
        .context("failed to instantiate component")?;
    store.data_mut().instance = Some(instance.clone());
    let func = {
        let mut exports = instance.exports(&mut store);
        if instance_name.is_empty() {
            exports.root()
        } else {
            exports
                .instance(instance_name)
                .with_context(|| format!("instance of `{instance_name}` not found"))?
        }
        .func(func_name)
        .with_context(|| format!("function `{func_name}` not found"))?
    };
    // TODO: process headers
    let conn = conn
        .connect(&nats, Bytes::new(), None)
        .await
        .context("failed to connect to peer")?;
    let (tx, mut root, mut child) = conn.split();
    let mut params = vec![Val::Bool(false); params_ty.len()];
    read_values(
        &mut store,
        &mut root,
        &mut child,
        payload,
        &mut params,
        params_ty,
    )
    .await
    .context("failed to decode parameters")?;
    let mut results = vec![Val::Bool(false); results_ty.len()];
    func.call_async(&mut store, &params, &mut results)
        .await
        .context("failed to call function")?;
    let mut payload = BytesMut::new();
    let futs = write_value_iter(
        &mut store,
        &mut payload,
        results.iter().zip(results_ty).map(Into::into),
    )
    .await
    .context("failed to encode results")?;
    let mut tx = tx.expect("reply missing for accepted request");
    let pollables = futs
        .into_pollables(&mut store, &tx)
        .context("failed to construct pollables")?;
    *tx.buffer_mut() = payload.freeze();
    tx.sized()
        .flush(&nats)
        .await
        .context("failed to send response")?;
    func.post_return_async(&mut store)
        .await
        .context("failed to perform post-return cleanup")?;
    handle_pollables(&mut store, &instance, &nats, exports, pollables)
        .await
        .context("failed to handle pollables")?;
    Ok(())
}

struct Server {
    guest_resources: HashSet<wrpc::nats::Listener>,
}

#[derive(Debug)]
enum FunctionExport {
    Method {
        receiver: GuestResourceType,
        params: Arc<Vec<ComponentType>>,
        results: Arc<Vec<ComponentType>>,
    },
    Static {
        params: Arc<Vec<ComponentType>>,
        results: Arc<Vec<ComponentType>>,
    },
}

impl FunctionExport {
    pub fn resolve(
        resolve: &wit_parser::Resolve,
        wit_parser::Function {
            params,
            results,
            kind,
            ..
        }: &wit_parser::Function,
    ) -> anyhow::Result<Self> {
        let mut params = params
            .iter()
            .map(|(_, ty)| resolve_type(resolve, ty))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("failed to resolve parameter types")?;
        let results = results
            .iter_types()
            .map(|ty| resolve_type(resolve, ty))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("failed to resolve result types")?;
        let results = Arc::new(results);
        match kind {
            wit_parser::FunctionKind::Method(_) => {
                if params.is_empty() {
                    bail!("method takes no parameters");
                }
                let ComponentType::Resource(ComponentResourceType::Guest(receiver)) =
                    params.remove(0)
                else {
                    bail!("first method parameter is not a guest resource");
                };
                let params = Arc::new(params);
                Ok(FunctionExport::Method {
                    receiver,
                    params,
                    results,
                })
            }
            _ => Ok(FunctionExport::Static {
                params: Arc::new(params),
                results,
            }),
        }
    }
}

fn function_exports<'a>(
    resolve: &wit_parser::Resolve,
    exports: impl IntoIterator<Item = (&'a WorldKey, &'a WorldItem)>,
) -> HashMap<String, HashMap<String, FunctionExport>> {
    exports
        .into_iter()
        .filter_map(|(wk, wi)| {
            let name = resolve.name_world_key(wk);
            match wi {
                WorldItem::Type(_ty) => {
                    trace!(name, "type export, skip");
                    None
                }
                WorldItem::Function(ty) => match FunctionExport::resolve(resolve, ty) {
                    Ok(ty) => Some((String::new(), HashMap::from([(name, ty)]))),
                    Err(err) => {
                        warn!(?err, "failed to resolve function export, skip");
                        None
                    }
                },
                WorldItem::Interface(interface_id) => {
                    let Some(wit_parser::Interface { functions, .. }) =
                        resolve.interfaces.get(*interface_id)
                    else {
                        warn!("component exports a non-existent interface, skip");
                        return None;
                    };
                    let functions = functions
                        .into_iter()
                        .filter_map(|(func_name, ty)| {
                            let ty = match FunctionExport::resolve(resolve, ty) {
                                Ok(ty) => ty,
                                Err(err) => {
                                    warn!(?err, "failed to resolve function export, skip");
                                    return None;
                                }
                            };
                            let func_name = if let FunctionExport::Method { .. } = ty {
                                let Some(func_name) = func_name.strip_prefix("[method]") else {
                                    error!("`[method]` prefix missing in method name, skip");
                                    return None;
                                };
                                func_name
                            } else {
                                func_name
                            };
                            Some((func_name.to_string(), ty))
                        })
                        .collect();
                    Some((name, functions))
                }
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .or_else(|_| tracing_subscriber::EnvFilter::try_new("info"))
                .expect("failed to setup env logging filter"),
        )
        .with(tracing_subscriber::fmt::layer().without_time())
        .init();

    let Args {
        nats,
        prefix,
        workload,
    } = Args::parse();
    let nats = async_nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;
    let nats = Arc::new(nats);

    let engine = Engine::new(
        wasmtime::Config::new()
            .async_support(true)
            .wasm_component_model(true),
    )
    .context("failed to initialize Wasmtime engine")?;

    let wasm = if workload.starts_with('.') {
        fs::read(&workload)
            .await
            .with_context(|| format!("failed to read relative path to workload `{workload}`"))
            .map(Workload::Binary)
    } else {
        Url::parse(&workload)
            .with_context(|| format!("failed to parse Wasm URL `{workload}`"))
            .map(Workload::Url)
    }?;
    let wasm = match wasm {
        Workload::Url(wasm) => match wasm.scheme() {
            "file" => {
                let wasm = wasm
                    .to_file_path()
                    .map_err(|_| anyhow!("failed to convert Wasm URL to file path"))?;
                fs::read(wasm)
                    .await
                    .context("failed to read Wasm from file URL")?
            }
            "http" | "https" => {
                let wasm = reqwest::get(wasm).await.context("failed to GET Wasm URL")?;
                let wasm = wasm.bytes().await.context("failed fetch Wasm from URL")?;
                wasm.to_vec()
            }
            scheme => bail!("URL scheme `{scheme}` not supported"),
        },
        Workload::Binary(wasm) => wasm,
    };
    let wasm = if wasmparser::Parser::is_core_wasm(&wasm) {
        wit_component::ComponentEncoder::default()
            .validate(true)
            .module(&wasm)
            .context("failed to set core component module")?
            .adapter(
                "wasi_snapshot_preview1",
                WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER,
            )
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let (resolve, world) =
        match wit_component::decode(&wasm).context("failed to decode WIT component")? {
            wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
            wit_component::DecodedWasm::WitPackage(..) => {
                bail!("binary-encoded WIT packages not supported")
            }
        };
    let resolve = Arc::new(resolve);

    let component = Component::new(&engine, wasm).context("failed to parse component")?;

    let mut linker: Linker<Ctx> = Linker::new(&engine);

    command::add_to_linker(&mut linker).context("failed to link `wasi:cli/command` interfaces")?;
    wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:http/types` interface")?;
    wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(&mut linker, |ctx| {
        ctx
    })
    .context("failed to link `wasi:http/outgoing-handler` interface")?;

    let World {
        exports, imports, ..
    } = resolve
        .worlds
        .iter()
        .find_map(|(id, w)| (id == world).then_some(w))
        .context("component world missing")?;

    let exports = Arc::new(function_exports(&resolve, exports));
    let guest_resources = Arc::new(polyfill(
        &component,
        &resolve,
        &exports,
        imports,
        &mut linker,
    ));
    let instance_pre = linker
        .instantiate_pre(&component)
        .context("failed to pre-instantiated component")?;

    let handlers = FuturesUnordered::new();
    for (instance_name, functions) in exports.as_ref() {
        let instance_name = Arc::new(instance_name.clone());
        for (func_name, export) in functions {
            let FunctionExport::Static { params, results } = export else {
                continue;
            };
            let mut path = String::with_capacity(
                prefix.len() + PROTOCOL.len() + instance_name.len() + func_name.len() + 3,
            );
            if !prefix.is_empty() {
                path.push_str(&prefix);
                path.push('.');
            }
            path.push_str(PROTOCOL);
            path.push('.');
            if !instance_name.is_empty() {
                path.push_str(&instance_name);
                path.push('/');
            }
            path.push_str(func_name);

            let engine = engine.clone();
            let instance_pre = instance_pre.clone();
            let func_name = func_name.to_string();

            let exports = Arc::clone(&exports);
            let guest_resources = Arc::clone(&guest_resources);
            let instance_name = Arc::clone(&instance_name);
            let nats = Arc::clone(&nats);
            let params = Arc::clone(params);
            let results = Arc::clone(results);

            handlers.push(spawn(async move {
                let lis = listen(&nats, path.clone())
                    .await
                    .with_context(|| format!("failed to listen on `{path}`"))?;
                lis.for_each_concurrent(None, |msg| async {
                    match msg {
                        Ok(req) => {
                            let reply = req.conn.peer_inbox().clone();
                            if let Err(err) = handle_connection(
                                Arc::clone(&nats),
                                &engine,
                                &instance_pre,
                                Arc::clone(&guest_resources),
                                &exports,
                                &instance_name,
                                &func_name,
                                &params,
                                &results,
                                req,
                            )
                            .await
                            {
                                warn!("failed to handle connection on `{path}`: {err:#}");
                                if let Err(publish_err) = nats
                                    .publish(format!("{reply}.error"), format!("{err:#}").into())
                                    .await
                                {
                                    warn!(?err, ?publish_err, "failed to send error to client");
                                }
                            }
                        }
                        Err(err) => {
                            warn!("failed to accept connection on `{path}`: {err:#}")
                        }
                    }
                })
                .await;
                Ok(())
            }))
        }
    }
    handlers
        .try_for_each_concurrent(None, |res: anyhow::Result<()>| async {
            if let Err(res) = res {
                error!("handler failed: {res}")
            }
            Ok(())
        })
        .await?;
    Ok(())
}
