#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

use core::borrow::Borrow;
use core::fmt;
use core::iter::zip;
use core::pin::pin;
use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures::future::try_join_all;
use tokio::io::AsyncWriteExt as _;
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn};
use uuid::Uuid;
use wasmtime::component::{
    Accessor, Func, Resource, ResourceAny, ResourceTable, ResourceType, Type, Val, types,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, Engine};
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing};

use crate::bindings::rpc::context::Context;
use crate::bindings::rpc::error::Error;

pub mod bindings;
mod codec;
pub mod paths;
mod polyfill;
pub mod rpc;
mod serve;

pub use codec::*;
pub use polyfill::*;
pub use serve::*;

// this returns the RPC name for a wasmtime function name.
// Unfortunately, the [`types::ComponentFunc`] does not include the kind information and we want to
// avoid (re-)parsing the WIT here.
fn rpc_func_name(name: &str) -> &str {
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

fn rpc_result_type<T: Borrow<Type>>(
    host_resources: &HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>,
    results_ty: impl IntoIterator<Item = T>,
) -> Option<Option<Type>> {
    let rpc_err_ty = host_resources
        .get("wrpc:rpc/error@0.2.0")
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
            Some(result_ty.ok())
        }
        _ => None,
    }
}

pub struct RemoteResource(pub Bytes);

/// Resource types, values of which are given special treatment by the wRPC value codec
#[derive(Clone, Debug, Default)]
pub struct ResourceTypes {
    /// Guest-exported resource types, handles of which are transmitted as references to
    /// [`SharedResourceTable`] entries
    pub guest: Arc<[ResourceType]>,
    /// Resource types bridged to wRPC `stream<u8>` (`wasi:io` `input-stream`), identified by
    /// their possibly-uninstantiated type
    #[cfg(feature = "wasmtime-wasi")]
    pub io_streams: Arc<[ResourceType]>,
}

/// A table of shared resources exported by the component
#[derive(Debug, Default)]
pub struct SharedResourceTable(HashMap<Uuid, ResourceAny>);

pub trait WrpcCtx<T: Invoke>: Send {
    /// Returns context to use for invocation
    fn context(&self) -> T::Context;

    /// Returns an [Invoke] implementation used to satisfy polyfilled imports
    fn client(&self) -> &T;

    /// Returns a table of shared exported resources
    fn shared_resources(&mut self) -> &mut SharedResourceTable;

    /// Optional invocation timeout. Polyfilled imports fail if the invocation is not finished
    /// within the returned [Duration], while `wrpc:rpc` invocations fail if they are not
    /// established within it. If this method returns [None], then no timeout will be used.
    fn timeout(&self) -> Option<Duration> {
        None
    }

    /// Maximum total number of indices in the async paths declared by a `wrpc:rpc`
    /// invocation, which bounds the memory used to track them. Invocations declaring more fail.
    fn max_path_indices(&self) -> usize {
        DEFAULT_MAX_PATH_INDICES
    }
}

/// Default of [`WrpcCtx::max_path_indices`]
pub const DEFAULT_MAX_PATH_INDICES: usize = 4096;

pub struct WrpcCtxView<'a, T: Invoke> {
    pub ctx: &'a mut dyn WrpcCtx<T>,
    pub table: &'a mut ResourceTable,
}

pub trait WrpcView: Send {
    type Invoke: Invoke;

    fn wrpc(&mut self) -> WrpcCtxView<'_, Self::Invoke>;
}

impl<T: WrpcView> WrpcView for &mut T {
    type Invoke = T::Invoke;

    fn wrpc(&mut self) -> WrpcCtxView<'_, Self::Invoke> {
        T::wrpc(self)
    }
}

pub trait WrpcViewExt: WrpcView {
    fn push_error(&mut self, error: Error) -> wasmtime::Result<Resource<Error>> {
        self.wrpc()
            .table
            .push(error)
            .context("failed to push error to table")
    }

    fn get_error(&mut self, error: &Resource<Error>) -> wasmtime::Result<&Error> {
        let error = self
            .wrpc()
            .table
            .get(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn get_error_mut(&mut self, error: &Resource<Error>) -> wasmtime::Result<&mut Error> {
        let error = self
            .wrpc()
            .table
            .get_mut(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn delete_error(&mut self, error: Resource<Error>) -> wasmtime::Result<Error> {
        let error = self
            .wrpc()
            .table
            .delete(error)
            .context("failed to delete error from table")?;
        Ok(error)
    }

    fn push_context(
        &mut self,
        cx: <Self::Invoke as Invoke>::Context,
    ) -> wasmtime::Result<Resource<Context>>
    where
        <Self::Invoke as Invoke>::Context: 'static,
    {
        self.wrpc()
            .table
            .push(Context(Box::new(cx)))
            .context("failed to push context to table")
    }

    fn delete_context(
        &mut self,
        cx: Resource<Context>,
    ) -> wasmtime::Result<<Self::Invoke as Invoke>::Context>
    where
        <Self::Invoke as Invoke>::Context: 'static,
    {
        let Context(cx) = self
            .wrpc()
            .table
            .delete(cx)
            .context("failed to delete context from table")?;
        let cx = cx
            .downcast()
            .map_err(|_| wasmtime::Error::msg("invalid context type"))?;
        Ok(*cx)
    }
}

impl<T: WrpcView> WrpcViewExt for T {}

/// Error type returned by [call]
pub enum CallError {
    Decode(wasmtime::Error),
    Encode(wasmtime::Error),
    Table(wasmtime::Error),
    Call(wasmtime::Error),
    TypeMismatch(wasmtime::Error),
    Write(wasmtime::Error),
    Flush(wasmtime::Error),
    Deferred(wasmtime::Error),
    PostReturn(wasmtime::Error),
    Guest(Error),
}

impl core::error::Error for CallError {}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Table(error)
            | CallError::Call(error)
            | CallError::TypeMismatch(error)
            | CallError::Write(error)
            | CallError::Flush(error)
            | CallError::Deferred(error)
            | CallError::PostReturn(error) => error.fmt(f),
            CallError::Guest(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Table(error)
            | CallError::Call(error)
            | CallError::TypeMismatch(error)
            | CallError::Write(error)
            | CallError::Flush(error)
            | CallError::Deferred(error)
            | CallError::PostReturn(error) => error.fmt(f),
            CallError::Guest(error) => error.fmt(f),
        }
    }
}

/// Calls the exported `func` with parameters decoded from `rx` and transmits the results on `tx`.
///
/// The `store` [`Accessor`] is used to call `func` concurrently, it can be acquired, for example,
/// using [`wasmtime::Store::run_concurrent`].
#[allow(clippy::too_many_arguments)]
pub async fn call<T>(
    mut store: &Accessor<T>,
    rx: Incoming,
    mut tx: Outgoing,
    resources: &ResourceTypes,
    host_resources: &HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>,
    params_ty: impl ExactSizeIterator<Item = &Type>,
    results_ty: &[Type],
    func: Func,
) -> Result<(), CallError>
where
    T: WrpcView + 'static,
{
    let mut params = vec![Val::Bool(false); params_ty.len()];
    let mut rx = pin!(rx);
    for (i, (v, ty)) in zip(&mut params, params_ty).enumerate() {
        read_value(&mut store, &mut rx, resources, v, ty, &[i])
            .await
            .with_context(|| format!("failed to decode parameter value {i}"))
            .map_err(CallError::Decode)?;
    }
    let mut results = vec![Val::Bool(false); results_ty.len()];
    func.call_concurrent(store, &params, &mut results)
        .await
        .context("failed to call function")
        .map_err(CallError::Call)?;

    let (buf, deferred) = store.with(|mut store| {
        let mut buf = BytesMut::default();
        let mut deferred = vec![];
        match (
            &rpc_result_type(host_resources, results_ty),
            results.as_slice(),
        ) {
            (None, results) => {
                for (i, (v, ty)) in zip(results, results_ty).enumerate() {
                    let mut enc = ValEncoder::new(store.as_context_mut(), ty, resources);
                    enc.encode(v, &mut buf)
                        .with_context(|| format!("failed to encode result value {i}"))
                        .map_err(CallError::Encode)?;
                    deferred.push(enc.deferred);
                }
            }
            // `result<_, rpc-eror>`
            (Some(None), [Val::Result(Ok(None))]) => {}
            // `result<T, rpc-eror>`
            (Some(Some(ty)), [Val::Result(Ok(Some(v)))]) => {
                let mut enc = ValEncoder::new(store.as_context_mut(), ty, resources);
                enc.encode(v, &mut buf)
                    .context("failed to encode result value 0")
                    .map_err(CallError::Encode)?;
                deferred.push(enc.deferred);
            }
            (Some(..), [Val::Result(Err(Some(err)))]) => {
                let Val::Resource(err) = &**err else {
                    return Err(CallError::TypeMismatch(wasmtime::Error::msg(
                        "RPC result error value is not a resource",
                    )));
                };
                let err = err
                    .try_into_resource(&mut store)
                    .context("RPC result error resource type mismatch")
                    .map_err(CallError::TypeMismatch)?;
                let err = store
                    .data_mut()
                    .delete_error(err)
                    .map_err(CallError::Table)?;
                return Err(CallError::Guest(err));
            }
            _ => {
                return Err(CallError::TypeMismatch(wasmtime::Error::msg(
                    "RPC result type mismatch",
                )));
            }
        }
        Ok((buf, deferred))
    })?;

    debug!("transmitting results");
    tx.write_bytes(buf.freeze())
        .await
        .context("failed to transmit results")
        .map_err(CallError::Write)?;
    tx.flush()
        .await
        .context("failed to flush outgoing stream")
        .map_err(CallError::Flush)?;
    try_join_all(
        zip(0.., deferred)
            .filter_map(|(i, f)| f.map(|f| (tx.index(&[i]), f)))
            .map(|(w, f)| async move {
                let w = w.map_err(wasmtime::Error::from)?;
                f(w).await
            }),
    )
    .await
    .map_err(CallError::Deferred)?;
    Ok(())
}

/// Recursively iterates the component item type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
pub fn collect_item_resource_exports(
    engine: &Engine,
    ty: types::ComponentItem,
    resources: &mut impl Extend<types::ResourceType>,
) {
    match ty {
        types::ComponentItem::ComponentFunc(_)
        | types::ComponentItem::CoreFunc(_)
        | types::ComponentItem::Module(_)
        | types::ComponentItem::Type(_) => {}
        types::ComponentItem::Component(ty) => {
            collect_component_resource_exports(engine, &ty, resources);
        }

        types::ComponentItem::ComponentInstance(ty) => {
            collect_instance_resource_exports(engine, &ty, resources);
        }
        types::ComponentItem::Resource(ty) => {
            debug!(?ty, "collect resource export");
            resources.extend([ty]);
        }
    }
}

/// Recursively iterates the instance type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
pub fn collect_instance_resource_exports(
    engine: &Engine,
    ty: &types::ComponentInstance,
    resources: &mut impl Extend<types::ResourceType>,
) {
    for (name, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
        trace!(name, ?ty, "collect instance item resource exports");
        collect_item_resource_exports(engine, ty, resources);
    }
}

/// Recursively iterates the component type and collects all exported resource types
#[instrument(level = "debug", skip_all)]
pub fn collect_component_resource_exports(
    engine: &Engine,
    ty: &types::Component,
    resources: &mut impl Extend<types::ResourceType>,
) {
    for (name, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
        trace!(name, ?ty, "collect component item resource exports");
        collect_item_resource_exports(engine, ty, resources);
    }
}

/// Iterates the component type and collects all imported resource types
#[instrument(level = "debug", skip_all)]
pub fn collect_component_resource_imports(
    engine: &Engine,
    ty: &types::Component,
    resources: &mut BTreeMap<Box<str>, HashMap<Box<str>, types::ResourceType>>,
) {
    for (name, types::ComponentExtern { ty, .. }) in ty.imports(engine) {
        match ty {
            types::ComponentItem::ComponentFunc(..)
            | types::ComponentItem::CoreFunc(..)
            | types::ComponentItem::Module(..)
            | types::ComponentItem::Type(..)
            | types::ComponentItem::Component(..) => {}
            types::ComponentItem::ComponentInstance(ty) => {
                let instance = name;
                for (name, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                    if let types::ComponentItem::Resource(ty) = ty {
                        debug!(instance, name, ?ty, "collect instance resource import");
                        if let Some(resources) = resources.get_mut(instance) {
                            resources.insert(name.into(), ty);
                        } else {
                            resources.insert(instance.into(), HashMap::from([(name.into(), ty)]));
                        }
                    }
                }
            }
            types::ComponentItem::Resource(ty) => {
                debug!(name, "collect component resource import");
                if let Some(resources) = resources.get_mut("") {
                    resources.insert(name.into(), ty);
                } else {
                    resources.insert("".into(), HashMap::from([(name.into(), ty)]));
                }
            }
        }
    }
}
