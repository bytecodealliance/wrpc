#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

//! wRPC integration for Wasmtime
//!
//! `call` serves an invocation by calling a component function export, which [`Exports`] looks up
//! by the instance and function names of the invocation. The host owns stores and instances, for
//! example, it instantiates the component for each invocation or serves all connections in one
//! instance from a single `run_concurrent` event loop, which guest-exported resources require.

use core::convert::Infallible;

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, instrument, trace};
use uuid::Uuid;
use wasmtime::Engine;
use wasmtime::component::{ResourceAny, ResourceType, types};

#[cfg(feature = "async")]
mod call;
mod codec;
mod export;

#[cfg(feature = "async")]
pub use call::*;
pub use codec::*;
pub use export::*;

/// Resource types, values of which are given special treatment by the wRPC value codec
#[derive(Clone, Debug, Default)]
pub struct ResourceTypes {
    /// Guest-exported resource types, handles of which are transmitted as references to
    /// [`SharedResourceTable`] entries
    pub guest: Arc<[ResourceType]>,
}

/// A table of shared resources exported by the component
#[derive(Debug, Default)]
pub struct SharedResourceTable(HashMap<Uuid, ResourceAny>);

pub struct WrpcCtxView<'a> {
    pub shared_resources: &'a mut SharedResourceTable,
}

pub trait WrpcView: Send {
    fn wrpc(&mut self) -> WrpcCtxView<'_>;
}

impl<T: WrpcView> WrpcView for &mut T {
    fn wrpc(&mut self) -> WrpcCtxView<'_> {
        T::wrpc(self)
    }
}

impl WrpcView for Infallible {
    fn wrpc(&mut self) -> WrpcCtxView<'_> {
        match *self {}
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
