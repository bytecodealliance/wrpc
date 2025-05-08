#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::iter::zip;
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::future::try_join_all;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{debug, trace};
use uuid::Uuid;
use wasmtime::component::{Func, Resource, ResourceAny, ResourceType, Type, Val};
use wasmtime::AsContextMut;
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
mod types;

pub use codec::*;
pub use polyfill::*;
pub use serve::*;
pub use types::*;

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

    fn push_error(&mut self, error: Error) -> anyhow::Result<Resource<Error>> {
        self.table()
            .push(error)
            .context("failed to push error to table")
    }

    fn get_error(&mut self, error: &Resource<Error>) -> anyhow::Result<&Error> {
        let error = self
            .table()
            .get(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn get_error_mut(&mut self, error: &Resource<Error>) -> anyhow::Result<&mut Error> {
        let error = self
            .table()
            .get_mut(error)
            .context("failed to get error from table")?;
        Ok(error)
    }

    fn delete_error(&mut self, error: Resource<Error>) -> anyhow::Result<Error> {
        let error = self
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
    C::Data: WasiView + WrpcView,
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
        CustomReturnType::new(host_resources, results_ty),
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
        (Some(CustomReturnType::Rpc(None)), [Val::Result(Ok(None))]) => {}
        // `result<T, rpc-eror>`
        (Some(CustomReturnType::Rpc(Some(ty))), [Val::Result(Ok(Some(v)))]) => {
            let mut enc = ValEncoder::new(store.as_context_mut(), &ty, guest_resources);
            enc.encode(v, &mut buf)
                .context("failed to encode result value 0")
                .map_err(CallError::Encode)?;
            deferred.push(enc.deferred);
        }
        (Some(CustomReturnType::Rpc(..)), [Val::Result(Err(Some(err)))]) => {
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
