use core::borrow::Borrow;
use core::ops::{Bound, RangeBounds};

use std::collections::btree_map;
use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, ensure};
use tracing::{debug, instrument, trace};
use wasmtime::component::types::{Component, ComponentFunc, ComponentInstance, ComponentItem};
use wasmtime::component::{ResourceType, Type};
use wasmtime::Engine;

use crate::bindings::rpc::error::Error;

mod dynamic;

struct DynamicReturn<const N: u8> {}
struct DynamicStream<const N: u8> {}
struct DynamicFuture<const N: u8> {}

// this returns the RPC name for a wasmtime function name.
// Unfortunately, the [`ComponentFunc`] does not include the kind information and we want to
// avoid (re-)parsing the WIT here.
pub(crate) fn rpc_func_name(name: &str) -> &str {
    if let Some(name) = name.strip_prefix("[constructor]") {
        name
    } else if let Some(name) = name.strip_prefix("[static]") {
        name
    } else if let Some(name) = name.strip_prefix("[method]") {
        name
    } else {
        name
    }
}

pub(crate) enum CustomReturnType {
    Rpc(Option<Type>),
    AsyncReturn(Option<Type>),
}

impl CustomReturnType {
    pub fn new<T: Borrow<Type>>(
        host_resources: &HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>,
        results_ty: impl IntoIterator<Item = T>,
    ) -> Option<Self> {
        let rpc_err_ty = host_resources
            .get("wrpc:rpc/error@0.1.0")
            .and_then(|instance| instance.get("error"));
        let mut results_ty = results_ty.into_iter();
        match (
            rpc_err_ty,
            results_ty.next().as_ref().map(Borrow::borrow),
            results_ty.next(),
        ) {
            (Some((guest_rpc_err_ty, host_rpc_err_ty)), Some(Type::Result(result_ty)), None)
                if *host_rpc_err_ty == ResourceType::host::<Error>()
                    && result_ty.err() == Some(Type::Own(*guest_rpc_err_ty)) =>
            {
                Some(Self::Rpc(result_ty.ok()))
            }
            _ => None,
        }
    }
}

/// Recursively iterates the component item type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
fn collect_item_resource_exports(
    engine: &Engine,
    ty: ComponentItem,
    resources: &mut impl Extend<ResourceType>,
) {
    match ty {
        ComponentItem::ComponentFunc(_)
        | ComponentItem::CoreFunc(_)
        | ComponentItem::Module(_)
        | ComponentItem::Type(_) => {}
        ComponentItem::Component(ty) => collect_component_resource_exports(engine, &ty, resources),

        ComponentItem::ComponentInstance(ty) => {
            collect_instance_resource_exports(engine, &ty, resources)
        }
        ComponentItem::Resource(ty) => {
            debug!(?ty, "collect resource export");
            resources.extend([ty])
        }
    }
}

/// Recursively iterates the instance type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
fn collect_instance_resource_exports(
    engine: &Engine,
    ty: &ComponentInstance,
    resources: &mut impl Extend<ResourceType>,
) {
    for (name, ty) in ty.exports(engine) {
        trace!(name, ?ty, "collect instance item resource exports");
        collect_item_resource_exports(engine, ty, resources);
    }
}

/// Recursively iterates the component type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
fn collect_component_resource_exports(
    engine: &Engine,
    ty: &Component,
    resources: &mut impl Extend<ResourceType>,
) {
    for (name, ty) in ty.exports(engine) {
        trace!(name, ?ty, "collect component item resource exports");
        collect_item_resource_exports(engine, ty, resources);
    }
}

#[derive(Clone, Debug, Default)]
pub struct WasiIoStreamResources {
    pub input_stream: Box<[ResourceType]>,
    pub output_stream: Box<[ResourceType]>,
}

impl WasiIoStreamResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        use wasmtime_wasi::bindings::io::streams;

        match ty {
            ty if self.input_stream.contains(ty) => {
                Some(ResourceType::host::<streams::InputStream>())
            }
            ty if self.output_stream.contains(ty) => {
                Some(ResourceType::host::<streams::OutputStream>())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WasiIoResources {
    pub error: Box<[ResourceType]>,
    pub pollable: Box<[ResourceType]>,
    pub input_stream: Box<[ResourceType]>,
    pub output_stream: Box<[ResourceType]>,
}

impl WasiIoResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        use wasmtime_wasi::bindings::io;

        match ty {
            ty if self.error.contains(ty) => Some(ResourceType::host::<io::error::Error>()),
            ty if self.input_stream.contains(ty) => {
                Some(ResourceType::host::<io::streams::InputStream>())
            }
            ty if self.output_stream.contains(ty) => {
                Some(ResourceType::host::<io::streams::OutputStream>())
            }
            ty if self.pollable.contains(ty) => Some(ResourceType::host::<io::poll::Pollable>()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WrpcRpcTransportResources {
    pub incoming_channel: Box<[ResourceType]>,
    pub outgoing_channel: Box<[ResourceType]>,
    pub invocation: Box<[ResourceType]>,
}

impl WrpcRpcTransportResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        use crate::bindings::rpc::transport;

        match ty {
            ty if self.incoming_channel.contains(ty) => {
                Some(ResourceType::host::<transport::IncomingChannel>())
            }
            ty if self.outgoing_channel.contains(ty) => {
                Some(ResourceType::host::<transport::OutgoingChannel>())
            }
            ty if self.invocation.contains(ty) => {
                Some(ResourceType::host::<transport::Invocation>())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WrpcRpcResources {
    pub context: Box<[ResourceType]>,
    pub error: Box<[ResourceType]>,
    pub incoming_channel: Box<[ResourceType]>,
    pub outgoing_channel: Box<[ResourceType]>,
    pub invocation: Box<[ResourceType]>,
}

impl WrpcRpcResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        use crate::bindings::rpc;

        match ty {
            ty if self.error.contains(ty) => Some(ResourceType::host::<rpc::error::Error>()),
            ty if self.context.contains(ty) => Some(ResourceType::host::<rpc::context::Context>()),
            ty if self.incoming_channel.contains(ty) => {
                Some(ResourceType::host::<rpc::transport::IncomingChannel>())
            }
            ty if self.outgoing_channel.contains(ty) => {
                Some(ResourceType::host::<rpc::transport::OutgoingChannel>())
            }
            ty if self.invocation.contains(ty) => {
                Some(ResourceType::host::<rpc::transport::Invocation>())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DynamicReturnType {
    Eq(Type),
    Borrowed(ResourceType),
}

#[derive(Clone, Debug, Default)]
pub struct DynamicResources {
    pub returns: Box<[(DynamicReturnType, Vec<ResourceType>)]>,
    pub futures: Box<[(Type, Vec<ResourceType>)]>,
    pub streams: Box<[(Type, Vec<ResourceType>)]>,
}

impl DynamicResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        for (i, (_, returns)) in self.returns.iter().enumerate() {
            if returns.contains(ty) {
                return i.try_into().map(dynamic::return_resource).ok();
            }
        }
        for (i, (_, futures)) in self.futures.iter().enumerate() {
            if futures.contains(ty) {
                return i.try_into().map(dynamic::future_resource).ok();
            }
        }
        for (i, (_, streams)) in self.streams.iter().enumerate() {
            if streams.contains(ty) {
                return i.try_into().map(dynamic::stream_resource).ok();
            }
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct DynamicResourceCtx {
    pub returns: Box<[DynamicReturnType]>,
    pub futures: Box<[Type]>,
    pub streams: Box<[Type]>,
}

#[derive(Clone, Debug, Default)]
pub struct WasiextDynamicTypeResources {
    pub async_return: Box<[ResourceType]>,
    pub dynamic_future: Box<[ResourceType]>,
    pub dynamic_stream: Box<[ResourceType]>,
    pub stream_send: Box<[ResourceType]>,
    pub future_send: Box<[ResourceType]>,
}

impl WasiextDynamicTypeResources {
    pub fn host_type(&self, ty: &ResourceType) -> Option<ResourceType> {
        // TODO: Set resource types
        match ty {
            ty if self.async_return.contains(ty) => Some(ResourceType::host::<()>()),
            ty if self.dynamic_future.contains(ty) => Some(ResourceType::host::<()>()),
            ty if self.dynamic_stream.contains(ty) => Some(ResourceType::host::<()>()),
            ty if self.stream_send.contains(ty) => Some(ResourceType::host::<()>()),
            ty if self.future_send.contains(ty) => Some(ResourceType::host::<()>()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ComponentTypeInfo {
    /// Resource types exported by the component, collected recursively. May contain duplicates.
    pub exported_resources: Vec<ResourceType>,
    /// Resource types imported by the component per top-level instance
    pub imported_resources: BTreeMap<Box<str>, HashMap<Box<str>, ResourceType>>,
    /// Component function types imported by the component per top-level instance
    pub imported_functions: BTreeMap<Box<str>, HashMap<Box<str>, ComponentFunc>>,
    /// Component types imported by the component per top-level instance
    pub imported_components: BTreeMap<Box<str>, ComponentTypeInfo>,
}

fn assert_subscribe_signature(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    io_pollables: &[ResourceType],
) -> anyhow::Result<()> {
    if let Some(ty) = instance.get(format!("[method]{name}.subscribe").as_str()) {
        let mut pty = ty.params();
        let (Some((_, Type::Borrow(..))), None) = (pty.next(), pty.next()) else {
            bail!("`subscribe` on resource `{name}` does not take the borrowed resource as the only parameter")
        };
        let mut ty = ty.results();
        let (Some(Type::Own(ty)), None) = (ty.next(), ty.next()) else {
            bail!("`subscribe` on resource `{name}` does not return a single value")
        };
        ensure!(
            io_pollables.contains(&ty),
            "`subscribe` on resource `{name}` does not return `wasi:io/poll.pollable`"
        );
    }
    Ok(())
}

fn assert_empty_constructor_signature(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
) -> anyhow::Result<()> {
    if let Some(ty) = instance.get(format!("[constructor]{name}").as_str()) {
        ensure!(
            ty.params().next().is_none(),
            "constructor of resource `{name}` must not take any arguments as parameters"
        );
        let mut ty = ty.results();
        let (Some(Type::Own(..)), None) = (ty.next(), ty.next()) else {
            bail!("constructor of resource `{name}` does not return an owned resource")
        };
    }
    Ok(())
}

fn async_return_constructor_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
) -> anyhow::Result<Option<(Type, ResourceType)>> {
    instance
        .get(format!("[constructor]{name}").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, pty)), None) = (pty.next(), pty.next()) else {
                bail!(
                    "constructor of resource `{name}` does not take a single argument as parameter"
                )
            };
            let mut rty = ty.results();
            let (Some(Type::Own(rty)), None) = (rty.next(), rty.next()) else {
                bail!("constructor of resource `{name}` does not return an owned resource")
            };
            Ok((pty, rty))
        })
        .transpose()
}

fn async_return_await_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
) -> anyhow::Result<Option<(ResourceType, Type)>> {
    instance
        .get(format!("[static]{name}.await").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, Type::Own(pty))), None) = (pty.next(), pty.next()) else {
                bail!("`await` on resource `{name}` does not take the owned resource as the only parameter")
            };
            let mut rty = ty.results();
            let (Some(rty), None) = (rty.next(), rty.next()) else {
                bail!("`await` on resource `{name}` does not return a single value")
            };
            Ok((pty, rty))
        })
        .transpose()
}

fn async_return_import_type(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    io_pollables: &[ResourceType],
) -> anyhow::Result<(ResourceType, DynamicReturnType)> {
    assert_subscribe_signature(instance, name, io_pollables)?;
    let constructor_ty = async_return_constructor_import(instance, name)?;
    let await_ty = async_return_await_import(instance, name)?;

    match (constructor_ty, await_ty) {
        (None, None) => {
            bail!("`{name}` resource imports neither a constructor, nor `await`")
        }
        (None, Some((resource_ty, ty))) | (Some((ty, resource_ty)), None) => {
            Ok((resource_ty, DynamicReturnType::Eq(ty)))
        }

        (Some((ret_ty, ret_resource_ty)), Some((rx_resource_ty, rx_ty)))
            if ret_ty == rx_ty && ret_resource_ty == rx_resource_ty =>
        {
            Ok((ret_resource_ty, DynamicReturnType::Eq(ret_ty)))
        }

        (
            Some((Type::Borrow(ret_ty), ret_resource_ty)),
            Some((rx_resource_ty, Type::Own(rx_ty))),
        ) if ret_ty == rx_ty && ret_resource_ty == rx_resource_ty => {
            Ok((ret_resource_ty, DynamicReturnType::Borrowed(ret_ty)))
        }
        (Some(..), Some(..)) => {
            bail!("`{name}` resource constructor and `await` type do not match")
        }
    }
}

fn dynamic_future_await_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
) -> anyhow::Result<Option<(ResourceType, Type)>> {
    instance
        .get(format!("[method]{name}.await").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, Type::Borrow(pty))), None) = (pty.next(), pty.next()) else {
                bail!("`await` on resource `{name}` does not take the borrowed resource as the only parameter")
            };
            let mut rty = ty.results();
            let (Some(Type::Option(rty)), None) = (rty.next(), rty.next()) else {
                bail!("`await` on resource `{name}` does not return a single optional value")
            };
            Ok((pty, rty.ty()))
        })
        .transpose()
}

fn dynamic_future_send_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    future_send: &[ResourceType],
) -> anyhow::Result<Option<(ResourceType, Type)>> {
    instance
        .get(format!("[static]{name}.send").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, Type::Own(pty0))), Some((_, pty1)), None) =
                (pty.next(), pty.next(), pty.next())
            else {
                bail!("`send` on resource `{name}` does not take the owned resource as the first and value as the second parameter")
            };
            let mut rty = ty.results();
            let (Some(Type::Own(ty)), None) = (rty.next(), rty.next()) else {
                bail!("`send` on resource `{name}` does not return a single value")
            };
            ensure!(
                future_send.contains(&ty),
                "`send` on resource `{name}` does not return `wasiext:dynamic/types.future-send`"
            );
            Ok((pty0, pty1))
        })
        .transpose()
}

fn dynamic_future_import_type(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    io_pollables: &[ResourceType],
    future_send: &[ResourceType],
) -> anyhow::Result<(ResourceType, Type)> {
    assert_subscribe_signature(instance, name, io_pollables)?;
    assert_empty_constructor_signature(instance, name)?;

    let await_ty = dynamic_future_await_import(instance, name)?;
    let send_ty = dynamic_future_send_import(instance, name, &future_send)?;

    match (send_ty, await_ty) {
        (None, None) => {
            bail!("dynamic future `{name}` resource imports neither `send`, nor `await`")
        }
        (None, Some((resource_ty, ty))) | (Some((resource_ty, ty)), None) => Ok((resource_ty, ty)),

        (Some((send_resource_ty, send_ty)), Some((rx_resource_ty, rx_ty)))
            if send_ty == rx_ty && send_resource_ty == rx_resource_ty =>
        {
            Ok((send_resource_ty, send_ty))
        }

        (Some(..), Some(..)) => {
            bail!("dynamic future `{name}` resource `send` and `await` types do not match")
        }
    }
}

fn dynamic_stream_receive_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
) -> anyhow::Result<Option<(ResourceType, Type)>> {
    instance
        .get(format!("[method]{name}.receive").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, Type::Borrow(pty))), Some((_, Type::U32)), None) = (pty.next(), pty.next(), pty.next()) else {
                bail!("`receive` on resource `{name}` does not take the borrowed resource as the first and u32 count as the second parameter")
            };
            let mut rty = ty.results();
            let (Some(Type::List(rty)), None) = (rty.next(), rty.next()) else {
                bail!("`receive` on resource `{name}` does not return a list values")
            };
            Ok((pty, rty.ty()))
        })
        .transpose()
}

fn dynamic_stream_send_import(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    stream_send: &[ResourceType],
) -> anyhow::Result<Option<(ResourceType, Type)>> {
    instance
        .get(format!("[method]{name}.send").as_str())
        .map(|ty| {
            let mut pty = ty.params();
            let (Some((_, Type::Borrow(pty0))), Some((_, Type::List(pty1))), None) =
                (pty.next(), pty.next(), pty.next())
            else {
                bail!("`send` on resource `{name}` does not take the borrowed resource as the first and list of values as the second parameter")
            };
            let mut rty = ty.results();
            let (Some(Type::Own(ty)), None) = (rty.next(), rty.next()) else {
                bail!("`send` on resource `{name}` does not return a single value")
            };
            ensure!(
                stream_send.contains(&ty),
                "`send` on resource `{name}` does not return `wasiext:dynamic/types.stream-send`"
            );
            Ok((pty0, pty1.ty()))
        })
        .transpose()
}

fn dynamic_stream_import_type(
    instance: &HashMap<Box<str>, ComponentFunc>,
    name: &str,
    io_pollables: &[ResourceType],
    stream_send: &[ResourceType],
) -> anyhow::Result<(ResourceType, Type)> {
    assert_subscribe_signature(instance, name, io_pollables)?;
    assert_empty_constructor_signature(instance, name)?;

    let receive_ty = dynamic_stream_receive_import(instance, name)?;
    let send_ty = dynamic_stream_send_import(instance, name, &stream_send)?;

    match (send_ty, receive_ty) {
        (None, None) => {
            bail!("dynamic stream `{name}` resource imports neither `send`, nor `receive`")
        }
        (None, Some((resource_ty, ty))) | (Some((resource_ty, ty)), None) => Ok((resource_ty, ty)),

        (Some((send_resource_ty, send_ty)), Some((rx_resource_ty, rx_ty)))
            if send_ty == rx_ty && send_resource_ty == rx_resource_ty =>
        {
            Ok((send_resource_ty, send_ty))
        }

        (Some(..), Some(..)) => {
            bail!("dynamic stream `{name}` resource `send` and `receive` types do not match")
        }
    }
}

impl ComponentTypeInfo {
    pub fn new(engine: &Engine, ty: &Component) -> Self {
        let mut exported_resources = Vec::default();
        let mut imported_resources = BTreeMap::<_, HashMap<_, _>>::default();
        let mut imported_functions = BTreeMap::<_, HashMap<_, _>>::default();
        let mut imported_components = BTreeMap::default();
        for (name, ty) in ty.imports(engine) {
            match ty {
                ComponentItem::CoreFunc(..)
                | ComponentItem::Module(..)
                | ComponentItem::Type(..) => {}
                ComponentItem::ComponentInstance(ty) => {
                    let instance = name;
                    for (name, ty) in ty.exports(engine) {
                        match ty {
                            ComponentItem::CoreFunc(..)
                            | ComponentItem::Module(..)
                            | ComponentItem::ComponentInstance(..)
                            | ComponentItem::Type(..) => {}
                            ComponentItem::ComponentFunc(ty) => {
                                debug!(instance, name, ?ty, "collect instance function import");
                                if let Some(imported_functions) =
                                    imported_functions.get_mut(instance)
                                {
                                    imported_functions.insert(name.into(), ty);
                                } else {
                                    imported_functions.insert(
                                        instance.into(),
                                        HashMap::from([(name.into(), ty)]),
                                    );
                                }
                            }
                            ComponentItem::Component(ty) => {
                                debug!(name, "collect instance component import");
                                imported_components.insert(name.into(), Self::new(engine, &ty));
                            }
                            ComponentItem::Resource(ty) => {
                                debug!(instance, name, ?ty, "collect instance resource import");
                                if let Some(imported_resources) =
                                    imported_resources.get_mut(instance)
                                {
                                    imported_resources.insert(name.into(), ty);
                                } else {
                                    imported_resources.insert(
                                        instance.into(),
                                        HashMap::from([(name.into(), ty)]),
                                    );
                                }
                            }
                        }
                    }
                }
                ComponentItem::ComponentFunc(ty) => {
                    debug!(name, "collect component function import");
                    if let Some(imported_functions) = imported_functions.get_mut("") {
                        imported_functions.insert(name.into(), ty);
                    } else {
                        imported_functions.insert("".into(), HashMap::from([(name.into(), ty)]));
                    }
                }
                ComponentItem::Component(ty) => {
                    debug!(name, "collect component component import");
                    imported_components.insert(name.into(), Self::new(engine, &ty));
                }
                ComponentItem::Resource(ty) => {
                    debug!(name, "collect component resource import");
                    if let Some(imported_resources) = imported_resources.get_mut("") {
                        imported_resources.insert(name.into(), ty);
                    } else {
                        imported_resources.insert("".into(), HashMap::from([(name.into(), ty)]));
                    }
                }
            }
        }
        collect_component_resource_exports(engine, ty, &mut exported_resources);
        Self {
            exported_resources,
            imported_resources,
            imported_functions,
            imported_components,
        }
    }

    pub fn imported_instance_resources(
        &self,
        range: impl RangeBounds<str>,
    ) -> btree_map::Range<'_, Box<str>, HashMap<Box<str>, ResourceType>> {
        self.imported_resources.range(range)
    }

    pub fn imported_wasi_io_error_resources(&self) -> Box<[ResourceType]> {
        self.imported_instance_resources((
            Bound::Included("wasi:io/error@0.2"),
            Bound::Excluded("wasi:io/error@0.3"),
        ))
        .flat_map(|(_, instance)| instance.get("error").copied())
        .collect()
    }

    pub fn imported_wasi_io_pollable_resources(&self) -> Box<[ResourceType]> {
        self.imported_instance_resources((
            Bound::Included("wasi:io/poll@0.2"),
            Bound::Excluded("wasi:io/poll@0.3"),
        ))
        .flat_map(|(_, instance)| instance.get("pollable").copied())
        .collect()
    }

    pub fn imported_wasi_io_stream_resources(&self) -> WasiIoStreamResources {
        let mut input_stream = Vec::default();
        let mut output_stream = Vec::default();
        for (_, instance) in self.imported_instance_resources((
            Bound::Included("wasi:io/streams@0.2"),
            Bound::Excluded("wasi:io/streams@0.3"),
        )) {
            if let Some(ty) = instance.get("input-stream") {
                input_stream.push(*ty);
            }
            if let Some(ty) = instance.get("output-stream") {
                output_stream.push(*ty);
            }
        }
        WasiIoStreamResources {
            input_stream: input_stream.into(),
            output_stream: output_stream.into(),
        }
    }

    pub fn imported_wasi_io_resources(&self) -> WasiIoResources {
        let error = self.imported_wasi_io_error_resources();
        let pollable = self.imported_wasi_io_pollable_resources();
        let WasiIoStreamResources {
            input_stream,
            output_stream,
        } = self.imported_wasi_io_stream_resources();
        WasiIoResources {
            error,
            pollable,
            input_stream,
            output_stream,
        }
    }

    pub fn imported_wrpc_rpc_error_resources(&self) -> Box<[ResourceType]> {
        self.imported_instance_resources((
            Bound::Included("wrpc:rpc/error@0.1"),
            Bound::Excluded("wrpc:rpc/error@0.2"),
        ))
        .flat_map(|(_, instance)| instance.get("error").copied())
        .collect()
    }

    pub fn imported_wrpc_rpc_context_resources(&self) -> Box<[ResourceType]> {
        self.imported_instance_resources((
            Bound::Included("wrpc:rpc/context@0.1"),
            Bound::Excluded("wrpc:rpc/context@0.2"),
        ))
        .flat_map(|(_, instance)| instance.get("context").copied())
        .collect()
    }

    pub fn imported_wrpc_rpc_transport_resources(&self) -> WrpcRpcTransportResources {
        let mut incoming_channel = Vec::default();
        let mut outgoing_channel = Vec::default();
        let mut invocation = Vec::default();
        for (_, instance) in self.imported_instance_resources((
            Bound::Included("wrpc:rpc/transport@0.1"),
            Bound::Excluded("wrpc:rpc/transport@0.2"),
        )) {
            if let Some(ty) = instance.get("incoming-channel") {
                incoming_channel.push(*ty);
            }
            if let Some(ty) = instance.get("outgoing-channel") {
                outgoing_channel.push(*ty);
            }
            if let Some(ty) = instance.get("invocation") {
                invocation.push(*ty);
            }
        }
        WrpcRpcTransportResources {
            incoming_channel: incoming_channel.into(),
            outgoing_channel: outgoing_channel.into(),
            invocation: invocation.into(),
        }
    }

    pub fn imported_wrpc_rpc_resources(&self) -> WrpcRpcResources {
        let error = self.imported_wrpc_rpc_error_resources();
        let context = self.imported_wrpc_rpc_context_resources();
        let WrpcRpcTransportResources {
            incoming_channel,
            outgoing_channel,
            invocation,
        } = self.imported_wrpc_rpc_transport_resources();
        WrpcRpcResources {
            error,
            context,
            incoming_channel,
            outgoing_channel,
            invocation,
        }
    }

    pub fn imported_wasiext_dynamic_type_resources(&self) -> WasiextDynamicTypeResources {
        let mut async_return = Vec::default();
        let mut dynamic_future = Vec::default();
        let mut dynamic_stream = Vec::default();
        let mut future_send = Vec::default();
        let mut stream_send = Vec::default();
        for (_, instance) in self.imported_instance_resources((
            Bound::Included("wasiext:dynamic/types@0.1"),
            Bound::Excluded("wasiext:dynamic/types@0.2"),
        )) {
            if let Some(ty) = instance.get("async-return") {
                async_return.push(*ty);
            }
            if let Some(ty) = instance.get("dynamic-future") {
                dynamic_future.push(*ty);
            }
            if let Some(ty) = instance.get("dynamic-stream") {
                dynamic_stream.push(*ty);
            }
            if let Some(ty) = instance.get("future-send") {
                future_send.push(*ty);
            }
            if let Some(ty) = instance.get("stream-send") {
                stream_send.push(*ty);
            }
        }
        WasiextDynamicTypeResources {
            async_return: async_return.into(),
            dynamic_future: dynamic_future.into(),
            dynamic_stream: dynamic_stream.into(),
            future_send: future_send.into(),
            stream_send: stream_send.into(),
        }
    }

    pub fn imported_dynamic_resources(
        &self,
        types: &WasiextDynamicTypeResources,
        io_pollables: &[ResourceType],
    ) -> anyhow::Result<DynamicResources> {
        let mut returns = Vec::<(_, Vec<_>)>::default();
        let mut futures = Vec::<(_, Vec<_>)>::default();
        let mut streams = Vec::<(_, Vec<_>)>::default();
        for instance in self.imported_functions.values() {
            'outer: for (name, ty) in instance {
                let Some(name) = name.strip_prefix("[static]") else {
                    continue;
                };
                let Some((name, "register-dynamic-type")) = name.split_once('.') else {
                    continue;
                };
                let mut ty = ty.results();
                let (Some(Type::Own(ty)), None) = (ty.next(), ty.next()) else {
                    continue;
                };
                if types.async_return.contains(&ty) {
                    let (resource_ty, ty) = async_return_import_type(instance, name, io_pollables)?;
                    for (rty, resources) in &mut returns {
                        if *rty == ty {
                            resources.push(resource_ty);
                            continue 'outer;
                        }
                    }
                    returns.push((ty, vec![resource_ty]));
                } else if types.dynamic_future.contains(&ty) {
                    let (resource_ty, ty) = dynamic_future_import_type(
                        instance,
                        name,
                        io_pollables,
                        &types.future_send,
                    )?;
                    for (rty, resources) in &mut futures {
                        if *rty == ty {
                            resources.push(resource_ty);
                            continue 'outer;
                        }
                    }
                    futures.push((ty, vec![resource_ty]));
                } else if types.dynamic_stream.contains(&ty) {
                    let (resource_ty, ty) = dynamic_stream_import_type(
                        instance,
                        name,
                        io_pollables,
                        &types.stream_send,
                    )?;
                    for (rty, resources) in &mut streams {
                        if *rty == ty {
                            resources.push(resource_ty);
                            continue 'outer;
                        }
                    }
                    streams.push((ty, vec![resource_ty]));
                }
            }
        }
        Ok(DynamicResources {
            returns: returns.into(),
            futures: futures.into(),
            streams: streams.into(),
        })
    }
}
