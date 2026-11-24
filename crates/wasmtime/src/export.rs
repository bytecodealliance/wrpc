use core::error::Error;
use core::fmt;

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, warn};
use wasmparser::names::split_canonical_version;
use wasmtime::component::{Component, ComponentExportIndex, Type, types};

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

fn instance_name(name: &str) -> String {
    if let Some((id, version)) = name.split_once('@')
        && let Some((version, ..)) = split_canonical_version(version)
    {
        format!("{id}@{version}")
    } else {
        name.into()
    }
}

/// Function export of a component
#[derive(Clone, Debug)]
pub struct Export {
    /// Index of the export, which [`Instance::get_func`](wasmtime::component::Instance::get_func)
    /// looks up the function by
    pub idx: ComponentExportIndex,
    /// Parameter types
    pub params: Arc<[Type]>,
    /// Result types
    pub results: Arc<[Type]>,
}

/// Error of collecting the function exports of a component
#[derive(Debug)]
pub enum ExportsError {
    /// An export listed by the component type has no export index
    NotFound(Box<str>),
    /// Two exports map to the same instance name, such as `a:b/c@1.0.0` and `a:b/c@1.1.0`
    Conflict {
        /// Instance name both exports map to
        instance: Box<str>,
        /// Name of the export collected first
        first: Box<str>,
        /// Name of the export collected second
        second: Box<str>,
    },
}

impl fmt::Display for ExportsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "export `{name}` not found"),
            Self::Conflict {
                instance,
                first,
                second,
            } => write!(
                f,
                "exports `{first}` and `{second}` both map to instance `{instance}`"
            ),
        }
    }
}

impl Error for ExportsError {}

/// Function exports of a component by the instance and function names invocations of them carry
#[derive(Clone, Debug)]
pub struct Exports(HashMap<Box<str>, HashMap<Box<str>, Export>>);

impl Exports {
    /// Collects the function exports of `component`
    ///
    /// # Errors
    ///
    /// Returns an error if an export cannot be looked up or if two exports map to the same
    /// instance name
    pub fn new(component: &Component) -> Result<Self, ExportsError> {
        let engine = component.engine();
        let mut exports = HashMap::<Box<str>, HashMap<Box<str>, Export>>::default();
        let mut insert = |instance: &str,
                          instance_idx: Option<&ComponentExportIndex>,
                          name: &str,
                          ty: types::ComponentFunc|
         -> Result<(), ExportsError> {
            let idx = component
                .get_export_index(instance_idx, name)
                .ok_or_else(|| ExportsError::NotFound(name.into()))?;
            let params: Arc<[_]> = ty.params().map(|(.., ty)| ty).collect();
            debug!(instance, name, "collected function export");
            exports.entry(instance.into()).or_default().insert(
                rpc_func_name(name).into(),
                Export {
                    idx,
                    params,
                    results: ty.results().collect(),
                },
            );
            Ok(())
        };
        let mut instances = HashMap::<String, &str>::default();
        for (name, types::ComponentExtern { ty, .. }) in component.component_type().exports(engine)
        {
            match ty {
                types::ComponentItem::ComponentFunc(ty) => insert("", None, name, ty)?,
                types::ComponentItem::ComponentInstance(ty) => {
                    let idx = component
                        .get_export_index(None, name)
                        .ok_or_else(|| ExportsError::NotFound(name.into()))?;
                    let instance = instance_name(name);
                    if let Some(prev) = instances.insert(instance.clone(), name) {
                        return Err(ExportsError::Conflict {
                            instance: instance.into(),
                            first: prev.into(),
                            second: name.into(),
                        });
                    }
                    for (func, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                        match ty {
                            types::ComponentItem::ComponentFunc(ty) => {
                                insert(&instance, Some(&idx), func, ty)?;
                            }
                            types::ComponentItem::Type(..) | types::ComponentItem::Resource(..) => {
                            }
                            _ => warn!(%instance, name = func, "export kind not supported"),
                        }
                    }
                }
                types::ComponentItem::Type(..) | types::ComponentItem::Resource(..) => {}
                _ => warn!(name, "export kind not supported"),
            }
        }
        Ok(Self(exports))
    }

    /// Returns the export invoked as function `func` on instance `instance`
    #[must_use]
    pub fn get(&self, instance: &str, func: &str) -> Option<&Export> {
        let funcs = self.0.get(instance)?;
        funcs.get(func)
    }
}
