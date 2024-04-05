use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use tracing::{error, instrument, trace, warn};

/// Dynamic resource type
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct DynamicResource {
    pub instance: String,
    pub name: String,
}

/// Resource type
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum Resource {
    Pollable,
    InputStream,
    OutputStream,
    Dynamic(Arc<DynamicResource>),
}

/// Value type
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum Type {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    List(Arc<Type>),
    Record(Arc<[Type]>),
    Tuple(Arc<[Type]>),
    Variant(Arc<[Option<Type>]>),
    Enum,
    Option(Arc<Type>),
    Result {
        ok: Option<Arc<Type>>,
        err: Option<Arc<Type>>,
    },
    Flags,
    Future(Option<Arc<Type>>),
    Stream(Option<Arc<Type>>),
    Resource(Resource),
}

fn resolve_iter<'a, C: FromIterator<Type>>(
    resolve: &wit_parser::Resolve,
    types: impl IntoIterator<Item = &'a wit_parser::Type>,
) -> anyhow::Result<C> {
    types
        .into_iter()
        .map(|ty| Type::resolve(resolve, ty))
        .collect()
}

fn resolve_optional(
    resolve: &wit_parser::Resolve,
    ty: &Option<wit_parser::Type>,
) -> anyhow::Result<Option<Type>> {
    ty.as_ref().map(|ty| Type::resolve(resolve, ty)).transpose()
}

impl Type {
    #[instrument(level = "trace", skip(resolve), ret)]
    pub fn resolve_def(
        resolve: &wit_parser::Resolve,
        ty: &wit_parser::TypeDef,
    ) -> anyhow::Result<Self> {
        use wit_parser::{
            Case, Field, Handle, Record, Result_, Stream, Tuple, TypeDef, TypeDefKind, TypeOwner,
            Variant,
        };

        match ty {
            TypeDef {
                kind: TypeDefKind::Record(Record { fields, .. }),
                ..
            } => resolve_iter(resolve, fields.iter().map(|Field { ty, .. }| ty)).map(Type::Record),
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
                        let instance = resolve.id_of_name(pkg, interface_name);
                        match (instance.as_str(), name.as_str()) {
                            ("wasi:io/poll@0.2.0", "pollable") => {
                                Ok(Self::Resource(Resource::Pollable))
                            }
                            ("wasi:io/streams@0.2.0", "input-stream") => {
                                Ok(Self::Resource(Resource::InputStream))
                            }
                            ("wasi:io/streams@0.2.0", "output-stream") => {
                                Ok(Self::Resource(Resource::OutputStream))
                            }
                            _ => Ok(Self::Resource(Resource::Dynamic(Arc::new(
                                DynamicResource {
                                    instance,
                                    name: name.to_string(),
                                },
                            )))),
                        }
                    }
                    _ => bail!("only resources owned by an interface are currently supported"),
                }
            }
            TypeDef {
                kind: TypeDefKind::Handle(Handle::Own(ty) | Handle::Borrow(ty)),
                ..
            } => {
                let ty = resolve
                    .types
                    .get(*ty)
                    .context("unknown handle inner type")?;
                Self::resolve_def(resolve, ty)
            }
            TypeDef {
                kind: TypeDefKind::Flags(..),
                ..
            } => Ok(Self::Flags),
            TypeDef {
                kind: TypeDefKind::Tuple(Tuple { types }),
                ..
            } => resolve_iter(resolve, types).map(Type::Tuple),
            TypeDef {
                kind: TypeDefKind::Variant(Variant { cases }),
                ..
            } => cases
                .iter()
                .map(|Case { ty, .. }| resolve_optional(resolve, ty))
                .collect::<anyhow::Result<_>>()
                .map(Type::Variant),
            TypeDef {
                kind: TypeDefKind::Enum(..),
                ..
            } => Ok(Type::Enum),
            TypeDef {
                kind: TypeDefKind::Option(ty),
                ..
            } => {
                let ty =
                    Self::resolve(resolve, ty).context("failed to resolve inner option type")?;
                Ok(Type::Option(Arc::new(ty)))
            }
            TypeDef {
                kind: TypeDefKind::Result(Result_ { ok, err }),
                ..
            } => {
                let ok = resolve_optional(resolve, ok)
                    .context("failed to resolve inner result `ok` variant type")?
                    .map(Arc::new);
                let err = resolve_optional(resolve, err)
                    .context("failed to resolve inner result `err` variant type")?
                    .map(Arc::new);
                Ok(Type::Result { ok, err })
            }
            TypeDef {
                kind: TypeDefKind::List(ty),
                ..
            } => Self::resolve(resolve, ty)
                .context("failed to resolve inner list type")
                .map(Arc::new)
                .map(Self::List),
            TypeDef {
                kind: TypeDefKind::Future(ty),
                ..
            } => {
                let ty = resolve_optional(resolve, ty)
                    .context("failed to resolve inner future type")?
                    .map(Arc::new);
                Ok(Type::Future(ty))
            }
            TypeDef {
                kind: TypeDefKind::Stream(Stream { element, end }),
                ..
            } => {
                ensure!(
                    end.is_none(),
                    "stream end elements are deprecated and will be removed in preview 3"
                );
                let element = resolve_optional(resolve, element)
                    .context("failed to resolve inner stream `element` type")?
                    .map(Arc::new);
                Ok(Type::Stream(element))
            }
            TypeDef {
                kind: TypeDefKind::Type(ty),
                ..
            } => Self::resolve(resolve, ty).context("failed to resolve inner handle type"),
            TypeDef {
                kind: TypeDefKind::Unknown,
                ..
            } => bail!("invalid type definition"),
        }
    }

    #[instrument(level = "trace", skip(resolve), ret)]
    pub fn resolve(resolve: &wit_parser::Resolve, ty: &wit_parser::Type) -> anyhow::Result<Self> {
        use wit_parser::Type;

        match ty {
            Type::Bool => Ok(Self::Bool),
            Type::U8 => Ok(Self::U8),
            Type::U16 => Ok(Self::U16),
            Type::U32 => Ok(Self::U32),
            Type::U64 => Ok(Self::U64),
            Type::S8 => Ok(Self::S8),
            Type::S16 => Ok(Self::S16),
            Type::S32 => Ok(Self::S32),
            Type::S64 => Ok(Self::S64),
            Type::F32 => Ok(Self::F32),
            Type::F64 => Ok(Self::F64),
            Type::Char => Ok(Self::Char),
            Type::String => Ok(Self::String),
            Type::Id(ty) => {
                let ty = resolve.types.get(*ty).context("unknown type")?;
                Self::resolve_def(resolve, ty)
            }
        }
    }
}

/// Dynamic function
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum DynamicFunction {
    Method {
        receiver: Arc<DynamicResource>,
        params: Arc<[Type]>,
        results: Arc<[Type]>,
    },
    Static {
        params: Arc<[Type]>,
        results: Arc<[Type]>,
    },
}

impl DynamicFunction {
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
            .map(|(_, ty)| Type::resolve(resolve, ty))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("failed to resolve parameter types")?;
        let results = results
            .iter_types()
            .map(|ty| Type::resolve(resolve, ty))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("failed to resolve result types")?;
        let results = results.into();
        match kind {
            wit_parser::FunctionKind::Method(_) => {
                if params.is_empty() {
                    bail!("method takes no parameters");
                }
                let Type::Resource(Resource::Dynamic(receiver)) = params.remove(0) else {
                    bail!("first method parameter is not a guest resource");
                };
                let params = params.into();
                Ok(DynamicFunction::Method {
                    receiver,
                    params,
                    results,
                })
            }
            _ => Ok(DynamicFunction::Static {
                params: params.into(),
                results,
            }),
        }
    }
}

pub fn function_exports<'a>(
    resolve: &wit_parser::Resolve,
    exports: impl IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
) -> HashMap<String, HashMap<String, DynamicFunction>> {
    use wit_parser::WorldItem;

    exports
        .into_iter()
        .filter_map(|(wk, wi)| {
            let name = resolve.name_world_key(wk);
            match wi {
                WorldItem::Type(_ty) => {
                    trace!(name, "type export, skip");
                    None
                }
                WorldItem::Function(ty) => match DynamicFunction::resolve(resolve, ty) {
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
                            let ty = match DynamicFunction::resolve(resolve, ty) {
                                Ok(ty) => ty,
                                Err(err) => {
                                    warn!(?err, "failed to resolve function export, skip");
                                    return None;
                                }
                            };
                            let func_name = if let DynamicFunction::Method { .. } = ty {
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

#[cfg(test)]
mod tests {
    #[cfg(feature = "serde")]
    #[test]
    fn serde() {
        use super::*;

        let buf = serde_json::to_vec(&Type::U8).unwrap();
        let ty: Type = serde_json::from_slice(&buf).unwrap();
        assert_eq!(ty, Type::U8)
    }
}
