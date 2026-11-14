#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

use core::fmt;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::{select, try_join};
use tracing::{debug, instrument, trace, warn};
use uuid::Uuid;
use wasmtime::Engine;
use wasmtime::component::{
    Accessor, Func, ResourceAny, ResourceTable, ResourceType, Type, Val, types,
};
use wasmtime::error::Context as _;
use wrpc_transport::serve;

mod codec;
pub mod paths;
mod serve;

pub use codec::*;
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

pub struct WrpcCtxView<'a> {
    pub shared_resources: &'a mut SharedResourceTable,
    pub table: &'a mut ResourceTable,
}

pub trait WrpcView: Send {
    fn wrpc(&mut self) -> WrpcCtxView<'_>;
}

impl<T: WrpcView> WrpcView for &mut T {
    fn wrpc(&mut self) -> WrpcCtxView<'_> {
        T::wrpc(self)
    }
}

/// Error type returned by [call]
pub enum CallError {
    Decode(wasmtime::Error),
    Encode(wasmtime::Error),
    Call(wasmtime::Error),
    Write(wasmtime::Error),
    Closed(wasmtime::Error),
}

impl core::error::Error for CallError {}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Call(error)
            | CallError::Write(error)
            | CallError::Closed(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Call(error)
            | CallError::Write(error)
            | CallError::Closed(error) => error.fmt(f),
        }
    }
}

/// Calls the exported `func` with parameters received on `rx` and transmits the results on `tx`.
///
/// The `store` [`Accessor`] is used to call `func` concurrently, it can be acquired, for example,
/// using [`wasmtime::Store::run_concurrent`].
pub async fn call<T>(
    store: &Accessor<T>,
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    resources: &ResourceTypes,
    params_ty: &[Type],
    results_ty: &[Type],
    func: Func,
) -> Result<(), CallError>
where
    T: WrpcView + 'static,
{
    let (errors_tx, mut errors) = mpsc::channel(1);
    let (params, outgoing) = serve(
        tx,
        rx,
        ValDecoder::new(store, params_ty, resources, errors_tx),
    );
    let call = async {
        let (params, rx) = params
            .await
            .map_err(|err| CallError::Decode(wasmtime::Error::from_anyhow(err)))?;
        try_join!(
            async {
                rx.await
                    .map_err(|err| CallError::Closed(wasmtime::Error::from_anyhow(err)))
            },
            async {
                let mut results = vec![Val::Bool(false); results_ty.len()];
                func.call_concurrent(store, &params, &mut results)
                    .await
                    .context("failed to call function")
                    .map_err(CallError::Call)?;
                let tx = outgoing.send(
                    TupleEncoder::new(store, results_ty, resources),
                    results.as_slice(),
                );
                let tx = tx
                    .context("failed to encode results")
                    .map_err(CallError::Encode)?;
                debug!("transmitting results");
                tx.await
                    .context("failed to transmit results")
                    .map_err(CallError::Write)
            },
        )?;
        Ok(())
    };
    select! {
        biased;
        Some(err) = errors.recv() => Err(err)
            .context("failed to receive asynchronous parameter values")
            .map_err(CallError::Decode),
        res = call => res,
    }
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
