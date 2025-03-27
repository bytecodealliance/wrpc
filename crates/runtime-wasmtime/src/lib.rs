#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

use core::time::Duration;
use core::{any::Any, iter::zip};
use core::{future::Future, pin::pin};

use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::future::try_join_all;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn};
use uuid::Uuid;
use wasmtime::component::{types, Func, Resource, ResourceAny, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine};
use wasmtime_wasi::{IoView, WasiView};
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

pub struct RemoteResource(pub Bytes);

/// A table of shared resources exported by the component
#[derive(Debug, Default)]
pub struct SharedResourceTable(HashMap<Uuid, ResourceAny>);

pub trait WrpcView: IoView + Send {
    type Invoke: Invoke;

    /// Returns context to use for invocation
    fn context(&self) -> <Self::Invoke as Invoke>::Context;

    /// Returns an [Invoke] implementation used to satisfy polyfilled imports
    fn client(&self) -> &Self::Invoke;

    /// Returns a table of shared exported resources
    fn shared_resources(&mut self) -> &mut SharedResourceTable;

    /// Optional invocation timeout, component will trap if invocation is not finished within the
    /// returned [Duration]. If this method returns [None], then no timeout will be used.
    fn timeout(&self) -> Option<Duration> {
        None
    }
}

impl<T: WrpcView> WrpcView for &mut T {
    type Invoke = T::Invoke;

    fn context(&self) -> <Self::Invoke as Invoke>::Context {
        (**self).context()
    }

    fn client(&self) -> &Self::Invoke {
        (**self).client()
    }

    fn shared_resources(&mut self) -> &mut SharedResourceTable {
        (**self).shared_resources()
    }

    fn timeout(&self) -> Option<Duration> {
        (**self).timeout()
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
        self.table()
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
            .table()
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
            .table()
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
        self.table()
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
            .table()
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
        self.table()
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
            .table()
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

    fn push_error(&mut self, error: anyhow::Error) -> anyhow::Result<Resource<Error>> {
        self.table()
            .push(Error(error))
            .context("failed to push error to table")
    }

    fn get_error(&mut self, error: &Resource<Error>) -> anyhow::Result<&anyhow::Error> {
        let Error(error) = self
            .table()
            .get(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn get_error_mut(&mut self, error: &Resource<Error>) -> anyhow::Result<&mut anyhow::Error> {
        let Error(error) = self
            .table()
            .get_mut(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn delete_error(&mut self, error: Resource<Error>) -> anyhow::Result<anyhow::Error> {
        let Error(error) = self
            .table()
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
        self.table()
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
            .table()
            .delete(cx)
            .context("failed to delete context from table")?;
        let cx = cx.downcast().map_err(|_| anyhow!("invalid context type"))?;
        Ok(*cx)
    }
}

impl<T: WrpcView> WrpcViewExt for T {}

pub async fn call<C, I, O>(
    mut store: C,
    rx: I,
    mut tx: O,
    params_ty: impl ExactSizeIterator<Item = &Type>,
    results_ty: impl ExactSizeIterator<Item = &Type>,
    func: Func,
    guest_resources: &[ResourceType],
) -> anyhow::Result<()>
where
    I: AsyncRead + wrpc_transport::Index<I> + Send + Sync + Unpin + 'static,
    O: AsyncWrite + wrpc_transport::Index<O> + Send + Sync + Unpin + 'static,
    C: AsContextMut,
    C::Data: WasiView + WrpcView,
{
    let mut params = vec![Val::Bool(false); params_ty.len()];
    let mut rx = pin!(rx);
    for (i, (v, ty)) in zip(&mut params, params_ty).enumerate() {
        read_value(&mut store, &mut rx, guest_resources, v, ty, &[i])
            .await
            .with_context(|| format!("failed to decode parameter value {i}"))?;
    }
    let mut results = vec![Val::Bool(false); results_ty.len()];
    func.call_async(&mut store, &params, &mut results)
        .await
        .context("failed to call function")?;
    let mut buf = BytesMut::default();
    let mut deferred = vec![];
    for (i, (ref v, ty)) in zip(results, results_ty).enumerate() {
        let mut enc = ValEncoder::new(store.as_context_mut(), ty, guest_resources);
        enc.encode(v, &mut buf)
            .with_context(|| format!("failed to encode result value {i}"))?;
        deferred.push(enc.deferred);
    }
    debug!("transmitting results");
    tx.write_all(&buf)
        .await
        .context("failed to transmit results")?;
    tx.flush()
        .await
        .context("failed to flush outgoing stream")?;
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
    .await?;
    func.post_return_async(&mut store)
        .await
        .context("failed to perform post-return cleanup")?;
    Ok(())
}

/// Recursively iterates the component item type and collects all exported resource types
#[instrument(level = "trace", skip_all)]
pub fn collect_item_resources(
    engine: &Engine,
    ty: types::ComponentItem,
    resources: &mut impl Extend<types::ResourceType>,
) {
    match ty {
        types::ComponentItem::ComponentFunc(_)
        | types::ComponentItem::CoreFunc(_)
        | types::ComponentItem::Module(_)
        | types::ComponentItem::Type(_) => {}
        types::ComponentItem::Component(ty) => collect_component_resources(engine, &ty, resources),
        types::ComponentItem::ComponentInstance(ty) => {
            collect_instance_resources(engine, &ty, resources);
        }
        types::ComponentItem::Resource(ty) => resources.extend([ty]),
    }
}

/// Recursively iterates the component type and collects all exported resource types
#[instrument(level = "trace", skip_all)]
pub fn collect_instance_resources(
    engine: &Engine,
    ty: &types::ComponentInstance,
    resources: &mut impl Extend<types::ResourceType>,
) {
    for (_, ty) in ty.exports(engine) {
        collect_item_resources(engine, ty, resources);
    }
}

/// Recursively iterates the component type and collects all exported resource types
#[instrument(level = "trace", skip_all)]
pub fn collect_component_resources(
    engine: &Engine,
    ty: &types::Component,
    resources: &mut impl Extend<types::ResourceType>,
) {
    for (_, ty) in ty.exports(engine) {
        collect_item_resources(engine, ty, resources);
    }
}
