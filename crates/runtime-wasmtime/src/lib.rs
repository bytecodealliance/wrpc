#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

use core::any::Any;
use core::borrow::Borrow;
use core::fmt;
use core::future::Future;
use core::iter::zip;
use core::pin::pin;
use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::future::try_join_all;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn};
use uuid::Uuid;
use wasmtime::component::{
    types, Func, Resource, ResourceAny, ResourceTable, ResourceType, Type, Val,
};
use wasmtime::{AsContextMut, Engine};
use wrpc_transport::Invoke;

use crate::bindings::rpc::context::Context;
use crate::bindings::rpc::error::Error;
use crate::bindings::rpc::transport::{IncomingChannel, Invocation, OutgoingChannel};

pub mod bindings;
mod codec;
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
            Some(result_ty.ok())
        }
        _ => None,
    }
}

pub struct RemoteResource(pub Bytes);

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

    /// Optional invocation timeout, component will trap if invocation is not finished within the
    /// returned [Duration]. If this method returns [None], then no timeout will be used.
    fn timeout(&self) -> Option<Duration> {
        None
    }
}

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
    fn push_invocation(
        &mut self,
        invocation: impl Future<
                Output = anyhow::Result<(
                    <Self::Invoke as Invoke>::Outgoing,
                    <Self::Invoke as Invoke>::Incoming,
                )>,
            > + Send
            + 'static,
    ) -> anyhow::Result<Resource<Invocation>> {
        self.wrpc()
            .table
            .push(Invocation::Future(Box::pin(async move {
                let res = invocation.await;
                Box::new(res) as Box<dyn Any + Send>
            })))
            .context("failed to push invocation to table")
    }

    fn get_invocation_result(
        &mut self,
        invocation: &Resource<Invocation>,
    ) -> anyhow::Result<
        Option<
            &Box<
                anyhow::Result<(
                    <Self::Invoke as Invoke>::Outgoing,
                    <Self::Invoke as Invoke>::Incoming,
                )>,
            >,
        >,
    > {
        let invocation = self
            .wrpc()
            .table
            .get(invocation)
            .context("failed to get invocation from table")?;
        match invocation {
            Invocation::Future(..) => Ok(None),
            Invocation::Ready(res) => {
                let res = res.downcast_ref().context("invalid invocation type")?;
                Ok(Some(res))
            }
        }
    }

    fn delete_invocation(
        &mut self,
        invocation: Resource<Invocation>,
    ) -> anyhow::Result<
        impl Future<
            Output = anyhow::Result<(
                <Self::Invoke as Invoke>::Outgoing,
                <Self::Invoke as Invoke>::Incoming,
            )>,
        >,
    > {
        let invocation = self
            .wrpc()
            .table
            .delete(invocation)
            .context("failed to delete invocation from table")?;
        Ok(async move {
            let res = match invocation {
                Invocation::Future(fut) => fut.await,
                Invocation::Ready(res) => res,
            };
            let res = res
                .downcast()
                .map_err(|_| anyhow!("invalid invocation type"))?;
            *res
        })
    }

    fn push_outgoing_channel(
        &mut self,
        outgoing: <Self::Invoke as Invoke>::Outgoing,
    ) -> anyhow::Result<Resource<OutgoingChannel>> {
        self.wrpc()
            .table
            .push(OutgoingChannel(Arc::new(std::sync::RwLock::new(Box::new(
                outgoing,
            )))))
            .context("failed to push outgoing channel to table")
    }

    fn delete_outgoing_channel(
        &mut self,
        outgoing: Resource<OutgoingChannel>,
    ) -> anyhow::Result<<Self::Invoke as Invoke>::Outgoing> {
        let OutgoingChannel(outgoing) = self
            .wrpc()
            .table
            .delete(outgoing)
            .context("failed to delete outgoing channel from table")?;
        let outgoing =
            Arc::into_inner(outgoing).context("outgoing channel has an active stream")?;
        let Ok(outgoing) = outgoing.into_inner() else {
            bail!("lock poisoned");
        };
        let outgoing = outgoing
            .downcast()
            .map_err(|_| anyhow!("invalid outgoing channel type"))?;
        Ok(*outgoing)
    }

    fn push_incoming_channel(
        &mut self,
        incoming: <Self::Invoke as Invoke>::Incoming,
    ) -> anyhow::Result<Resource<IncomingChannel>> {
        self.wrpc()
            .table
            .push(IncomingChannel(Arc::new(std::sync::RwLock::new(Box::new(
                incoming,
            )))))
            .context("failed to push incoming channel to table")
    }

    fn delete_incoming_channel(
        &mut self,
        incoming: Resource<IncomingChannel>,
    ) -> anyhow::Result<<Self::Invoke as Invoke>::Incoming> {
        let IncomingChannel(incoming) = self
            .wrpc()
            .table
            .delete(incoming)
            .context("failed to delete incoming channel from table")?;
        let incoming =
            Arc::into_inner(incoming).context("incoming channel has an active stream")?;
        let Ok(incoming) = incoming.into_inner() else {
            bail!("lock poisoned");
        };
        let incoming = incoming
            .downcast()
            .map_err(|_| anyhow!("invalid incoming channel type"))?;
        Ok(*incoming)
    }

    fn push_error(&mut self, error: Error) -> anyhow::Result<Resource<Error>> {
        self.wrpc()
            .table
            .push(error)
            .context("failed to push error to table")
    }

    fn get_error(&mut self, error: &Resource<Error>) -> anyhow::Result<&Error> {
        let error = self
            .wrpc()
            .table
            .get(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn get_error_mut(&mut self, error: &Resource<Error>) -> anyhow::Result<&mut Error> {
        let error = self
            .wrpc()
            .table
            .get_mut(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn delete_error(&mut self, error: Resource<Error>) -> anyhow::Result<Error> {
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
    ) -> anyhow::Result<Resource<Context>>
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
    ) -> anyhow::Result<<Self::Invoke as Invoke>::Context>
    where
        <Self::Invoke as Invoke>::Context: 'static,
    {
        let Context(cx) = self
            .wrpc()
            .table
            .delete(cx)
            .context("failed to delete context from table")?;
        let cx = cx.downcast().map_err(|_| anyhow!("invalid context type"))?;
        Ok(*cx)
    }
}

impl<T: WrpcView> WrpcViewExt for T {}

/// Error type returned by [call]
pub enum CallError {
    Decode(anyhow::Error),
    Encode(anyhow::Error),
    Table(anyhow::Error),
    Call(anyhow::Error),
    TypeMismatch(anyhow::Error),
    Write(anyhow::Error),
    Flush(anyhow::Error),
    Deferred(anyhow::Error),
    PostReturn(anyhow::Error),
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

#[allow(clippy::too_many_arguments)]
pub async fn call<C, I, O>(
    mut store: C,
    rx: I,
    mut tx: O,
    guest_resources: &[ResourceType],
    host_resources: &HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>,
    params_ty: impl ExactSizeIterator<Item = &Type>,
    results_ty: &[Type],
    func: Func,
) -> Result<(), CallError>
where
    I: AsyncRead + wrpc_transport::Index<I> + Send + Sync + Unpin + 'static,
    O: AsyncWrite + wrpc_transport::Index<O> + Send + Sync + Unpin + 'static,
    C: AsContextMut,
    C::Data: WrpcView,
{
    let mut params = vec![Val::Bool(false); params_ty.len()];
    let mut rx = pin!(rx);
    for (i, (v, ty)) in zip(&mut params, params_ty).enumerate() {
        read_value(&mut store, &mut rx, guest_resources, v, ty, &[i])
            .await
            .with_context(|| format!("failed to decode parameter value {i}"))
            .map_err(CallError::Decode)?;
    }
    let mut results = vec![Val::Bool(false); results_ty.len()];
    func.call_async(&mut store, &params, &mut results)
        .await
        .context("failed to call function")
        .map_err(CallError::Call)?;

    let mut buf = BytesMut::default();
    let mut deferred = vec![];
    match (
        &rpc_result_type(host_resources, results_ty),
        results.as_slice(),
    ) {
        (None, results) => {
            for (i, (v, ty)) in zip(results, results_ty).enumerate() {
                let mut enc = ValEncoder::new(store.as_context_mut(), ty, guest_resources);
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
            let mut enc = ValEncoder::new(store.as_context_mut(), ty, guest_resources);
            enc.encode(v, &mut buf)
                .context("failed to encode result value 0")
                .map_err(CallError::Encode)?;
            deferred.push(enc.deferred);
        }
        (Some(..), [Val::Result(Err(Some(err)))]) => {
            let Val::Resource(err) = &**err else {
                return Err(CallError::TypeMismatch(anyhow!(
                    "RPC result error value is not a resource"
                )));
            };
            let mut store = store.as_context_mut();
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
        _ => return Err(CallError::TypeMismatch(anyhow!("RPC result type mismatch"))),
    }

    debug!("transmitting results");
    tx.write_all(&buf)
        .await
        .context("failed to transmit results")
        .map_err(CallError::Write)?;
    tx.flush()
        .await
        .context("failed to flush outgoing stream")
        .map_err(CallError::Flush)?;
    if let Err(err) = tx.shutdown().await {
        trace!(?err, "failed to shutdown outgoing stream");
    }
    try_join_all(
        zip(0.., deferred)
            .filter_map(|(i, f)| f.map(|f| (tx.index(&[i]), f)))
            .map(|(w, f)| async move {
                let w = w?;
                f(w).await
            }),
    )
    .await
    .map_err(CallError::Deferred)?;
    func.post_return_async(&mut store)
        .await
        .context("failed to perform post-return cleanup")
        .map_err(CallError::PostReturn)?;
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
            collect_component_resource_exports(engine, &ty, resources)
        }

        types::ComponentItem::ComponentInstance(ty) => {
            collect_instance_resource_exports(engine, &ty, resources)
        }
        types::ComponentItem::Resource(ty) => {
            debug!(?ty, "collect resource export");
            resources.extend([ty])
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
    for (name, ty) in ty.exports(engine) {
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
    for (name, ty) in ty.exports(engine) {
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
    for (name, ty) in ty.imports(engine) {
        match ty {
            types::ComponentItem::ComponentFunc(..)
            | types::ComponentItem::CoreFunc(..)
            | types::ComponentItem::Module(..)
            | types::ComponentItem::Type(..)
            | types::ComponentItem::Component(..) => {}
            types::ComponentItem::ComponentInstance(ty) => {
                let instance = name;
                for (name, ty) in ty.exports(engine) {
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
