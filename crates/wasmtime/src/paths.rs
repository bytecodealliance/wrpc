//! Computation of wRPC asynchronous subscription paths from Wasmtime component types.
//!
//! When serving a component function over wRPC, the transport must be told which
//! nested positions of the parameters carry asynchronous values (streams and
//! futures) so it can subscribe to their data channels. `wasi:io`
//! `input-stream`/`output-stream` resources are bridged to wRPC `stream<u8>` by
//! this runtime, so they are treated as asynchronous here as well.
//!
//! Note that at serve time the component has not been instantiated, so a
//! `wasi:io` stream parameter appears as the component's own *uninstantiated*
//! resource type rather than the host [`DynInputStream`]/[`DynOutputStream`]
//! type. We therefore identify those resources by collecting the component's
//! `wasi:io/streams` imports up front and comparing against them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use wasmtime::component::types::{self, Type};
use wasmtime::component::ResourceType;
use wasmtime::Engine;

/// Collect the (uninstantiated) resource types of the component's imported
/// `wasi:io/streams` `input-stream` and `output-stream`, against which
/// parameter resource types can be compared at serve time.
///
/// A `Vec` is used rather than a set because [`ResourceType`] is neither `Ord`
/// nor `Hash`; the collection holds at most two entries.
pub fn wasi_io_stream_resources(
    engine: &Engine,
    component: &types::Component,
) -> Vec<ResourceType> {
    let mut imports = BTreeMap::new();
    crate::collect_component_resource_imports(engine, component, &mut imports);
    let mut out = Vec::new();
    for (instance, resources) in imports {
        // Instance names are versioned, e.g. `wasi:io/streams@0.2.6`.
        let base = instance.split('@').next().unwrap_or(&instance);
        if base != "wasi:io/streams" {
            continue;
        }
        for (name, ty) in resources {
            if (&*name == "input-stream" || &*name == "output-stream") && !out.contains(&ty) {
                out.push(ty);
            }
        }
    }
    out
}

/// Compute the set of nested asynchronous paths within a single value type, and
/// whether the type *itself* is asynchronous (a stream or future).
///
/// Mirrors `wrpc_introspect::async_paths_ty`, but operates over Wasmtime's
/// component [`Type`] rather than WIT types. `streams` is the set of resource
/// types that are bridged to wRPC `stream<u8>` (see [`wasi_io_stream_resources`]).
fn async_paths(ty: &Type, streams: &[ResourceType]) -> (BTreeSet<VecDeque<Option<usize>>>, bool) {
    let mut paths = BTreeSet::new();
    match ty {
        Type::List(ty) => {
            let (nested, fut) = async_paths(&ty.ty(), streams);
            for mut path in nested {
                path.push_front(None);
                paths.insert(path);
            }
            if fut {
                paths.insert(VecDeque::from([None]));
            }
            (paths, false)
        }
        Type::Option(ty) => async_paths(&ty.ty(), streams),
        Type::Result(ty) => {
            let mut is_fut = false;
            if let Some(ty) = ty.ok() {
                let (nested, fut) = async_paths(&ty, streams);
                paths.extend(nested);
                is_fut |= fut;
            }
            if let Some(ty) = ty.err() {
                let (nested, fut) = async_paths(&ty, streams);
                paths.extend(nested);
                is_fut |= fut;
            }
            (paths, is_fut)
        }
        Type::Variant(ty) => {
            let mut is_fut = false;
            for case in ty.cases() {
                if let Some(ty) = case.ty {
                    let (nested, fut) = async_paths(&ty, streams);
                    paths.extend(nested);
                    is_fut |= fut;
                }
            }
            (paths, is_fut)
        }
        Type::Tuple(ty) => {
            for (i, ty) in ty.types().enumerate() {
                let (nested, fut) = async_paths(&ty, streams);
                for mut path in nested {
                    path.push_front(Some(i));
                    paths.insert(path);
                }
                if fut {
                    paths.insert(VecDeque::from([Some(i)]));
                }
            }
            (paths, false)
        }
        Type::Record(ty) => {
            for (i, field) in ty.fields().enumerate() {
                let (nested, fut) = async_paths(&field.ty, streams);
                for mut path in nested {
                    path.push_front(Some(i));
                    paths.insert(path);
                }
                if fut {
                    paths.insert(VecDeque::from([Some(i)]));
                }
            }
            (paths, false)
        }
        Type::Future(ty) => {
            if let Some(ty) = ty.ty() {
                (paths, _) = async_paths(&ty, streams);
            }
            (paths, true)
        }
        Type::Stream(ty) => {
            if let Some(ty) = ty.ty() {
                let (nested, fut) = async_paths(&ty, streams);
                for mut path in nested {
                    path.push_front(None);
                    paths.insert(path);
                }
                if fut {
                    paths.insert(VecDeque::from([None]));
                }
            }
            (paths, true)
        }
        Type::Own(ty) | Type::Borrow(ty) if streams.contains(ty) => {
            // `wasi:io` streams are sent/received as wRPC `stream<u8>`.
            (paths, true)
        }
        _ => (paths, false),
    }
}

/// Compute the wRPC subscription paths for a function's parameter list.
///
/// Each parameter is treated as an element of a top-level tuple: a parameter at
/// index `i` whose type carries asynchronous data contributes paths prefixed
/// with `Some(i)`.
pub(crate) fn params_async_paths<'a>(
    params: impl IntoIterator<Item = &'a Type>,
    streams: &[ResourceType],
) -> Arc<[Box<[Option<usize>]>]> {
    let mut out = BTreeSet::new();
    for (i, ty) in params.into_iter().enumerate() {
        let (nested, fut) = async_paths(ty, streams);
        for mut path in nested {
            path.push_front(Some(i));
            out.insert(path);
        }
        if fut {
            out.insert(VecDeque::from([Some(i)]));
        }
    }
    out.into_iter()
        .map(|path| path.into_iter().collect::<Box<[_]>>())
        .collect()
}
