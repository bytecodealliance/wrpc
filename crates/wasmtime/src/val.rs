use core::borrow::Borrow;
use core::fmt::{self, Debug, Formatter};
use core::iter::zip;

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{self, ResourceAny, Type};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut, bail};

#[cfg(feature = "async")]
mod concurrent;

/// A component value held outside of a store, mirroring [`component::Val`].
///
/// Values without resources, `future`s and `stream`s convert from and into [`component::Val`]
/// using [`TryFrom`], the others using [`lift`] and [`lower`].
pub enum Val {
    Bool(bool),
    S8(i8),
    U8(u8),
    S16(i16),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Float32(f32),
    Float64(f64),
    Char(char),
    String(String),
    List(Vec<Val>),
    Record(Vec<(String, Val)>),
    Tuple(Vec<Val>),
    Variant(String, Option<Box<Val>>),
    Enum(String),
    Option(Option<Box<Val>>),
    Result(Result<Option<Box<Val>>, Option<Box<Val>>>),
    Flags(Vec<String>),
    /// Resource handle, an opaque blob defined by the application
    Resource(Bytes),
    /// `future`, which resolves to its payload, if it has one
    Future(BoxFuture<'static, std::io::Result<Option<Val>>>),
    /// `stream` of chunks of elements
    Stream(BoxStream<'static, Vec<Val>>),
}

impl Debug for Val {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(v) => f.debug_tuple("Bool").field(v).finish(),
            Self::S8(v) => f.debug_tuple("S8").field(v).finish(),
            Self::U8(v) => f.debug_tuple("U8").field(v).finish(),
            Self::S16(v) => f.debug_tuple("S16").field(v).finish(),
            Self::U16(v) => f.debug_tuple("U16").field(v).finish(),
            Self::S32(v) => f.debug_tuple("S32").field(v).finish(),
            Self::U32(v) => f.debug_tuple("U32").field(v).finish(),
            Self::S64(v) => f.debug_tuple("S64").field(v).finish(),
            Self::U64(v) => f.debug_tuple("U64").field(v).finish(),
            Self::Float32(v) => f.debug_tuple("Float32").field(v).finish(),
            Self::Float64(v) => f.debug_tuple("Float64").field(v).finish(),
            Self::Char(v) => f.debug_tuple("Char").field(v).finish(),
            Self::String(v) => f.debug_tuple("String").field(v).finish(),
            Self::List(v) => f.debug_tuple("List").field(v).finish(),
            Self::Record(v) => f.debug_tuple("Record").field(v).finish(),
            Self::Tuple(v) => f.debug_tuple("Tuple").field(v).finish(),
            Self::Variant(case, v) => f.debug_tuple("Variant").field(case).field(v).finish(),
            Self::Enum(v) => f.debug_tuple("Enum").field(v).finish(),
            Self::Option(v) => f.debug_tuple("Option").field(v).finish(),
            Self::Result(v) => f.debug_tuple("Result").field(v).finish(),
            Self::Flags(v) => f.debug_tuple("Flags").field(v).finish(),
            Self::Resource(v) => f.debug_tuple("Resource").field(v).finish(),
            Self::Future(..) => f.debug_tuple("Future").finish_non_exhaustive(),
            Self::Stream(..) => f.debug_tuple("Stream").finish_non_exhaustive(),
        }
    }
}

fn convert_payload<T, U: TryFrom<T>>(v: Option<Box<T>>) -> Result<Option<Box<U>>, U::Error> {
    let Some(v) = v else {
        return Ok(None);
    };
    let v = U::try_from(*v)?;
    Ok(Some(Box::new(v)))
}

fn convert_field<T, U: TryFrom<T>>((name, v): (String, T)) -> Result<(String, U), U::Error> {
    let v = U::try_from(v)?;
    Ok((name, v))
}

impl TryFrom<Val> for component::Val {
    type Error = wasmtime::Error;

    fn try_from(v: Val) -> wasmtime::Result<Self> {
        match v {
            Val::Bool(v) => Ok(Self::Bool(v)),
            Val::S8(v) => Ok(Self::S8(v)),
            Val::U8(v) => Ok(Self::U8(v)),
            Val::S16(v) => Ok(Self::S16(v)),
            Val::U16(v) => Ok(Self::U16(v)),
            Val::S32(v) => Ok(Self::S32(v)),
            Val::U32(v) => Ok(Self::U32(v)),
            Val::S64(v) => Ok(Self::S64(v)),
            Val::U64(v) => Ok(Self::U64(v)),
            Val::Float32(v) => Ok(Self::Float32(v)),
            Val::Float64(v) => Ok(Self::Float64(v)),
            Val::Char(v) => Ok(Self::Char(v)),
            Val::String(v) => Ok(Self::String(v)),
            Val::List(vs) => {
                let vs = vs
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(Self::List(vs))
            }
            Val::Record(vs) => {
                let vs = vs
                    .into_iter()
                    .map(convert_field)
                    .collect::<Result<_, _>>()?;
                Ok(Self::Record(vs))
            }
            Val::Tuple(vs) => {
                let vs = vs
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(Self::Tuple(vs))
            }
            Val::Variant(case, v) => {
                let v = convert_payload(v)?;
                Ok(Self::Variant(case, v))
            }
            Val::Enum(v) => Ok(Self::Enum(v)),
            Val::Option(v) => {
                let v = convert_payload(v)?;
                Ok(Self::Option(v))
            }
            Val::Result(Ok(v)) => {
                let v = convert_payload(v)?;
                Ok(Self::Result(Ok(v)))
            }
            Val::Result(Err(v)) => {
                let v = convert_payload(v)?;
                Ok(Self::Result(Err(v)))
            }
            Val::Flags(vs) => Ok(Self::Flags(vs)),
            Val::Resource(..) => bail!("resources require a store"),
            Val::Future(..) => bail!("`future`s require a store"),
            Val::Stream(..) => bail!("`stream`s require a store"),
        }
    }
}

impl TryFrom<component::Val> for Val {
    type Error = wasmtime::Error;

    fn try_from(v: component::Val) -> wasmtime::Result<Self> {
        match v {
            component::Val::Bool(v) => Ok(Self::Bool(v)),
            component::Val::S8(v) => Ok(Self::S8(v)),
            component::Val::U8(v) => Ok(Self::U8(v)),
            component::Val::S16(v) => Ok(Self::S16(v)),
            component::Val::U16(v) => Ok(Self::U16(v)),
            component::Val::S32(v) => Ok(Self::S32(v)),
            component::Val::U32(v) => Ok(Self::U32(v)),
            component::Val::S64(v) => Ok(Self::S64(v)),
            component::Val::U64(v) => Ok(Self::U64(v)),
            component::Val::Float32(v) => Ok(Self::Float32(v)),
            component::Val::Float64(v) => Ok(Self::Float64(v)),
            component::Val::Char(v) => Ok(Self::Char(v)),
            component::Val::String(v) => Ok(Self::String(v)),
            component::Val::List(vs) => {
                let vs = vs
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(Self::List(vs))
            }
            component::Val::Record(vs) => {
                let vs = vs
                    .into_iter()
                    .map(convert_field)
                    .collect::<Result<_, _>>()?;
                Ok(Self::Record(vs))
            }
            component::Val::Tuple(vs) => {
                let vs = vs
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(Self::Tuple(vs))
            }
            component::Val::Variant(case, v) => {
                let v = convert_payload(v)?;
                Ok(Self::Variant(case, v))
            }
            component::Val::Enum(v) => Ok(Self::Enum(v)),
            component::Val::Option(v) => {
                let v = convert_payload(v)?;
                Ok(Self::Option(v))
            }
            component::Val::Result(Ok(v)) => {
                let v = convert_payload(v)?;
                Ok(Self::Result(Ok(v)))
            }
            component::Val::Result(Err(v)) => {
                let v = convert_payload(v)?;
                Ok(Self::Result(Err(v)))
            }
            component::Val::Flags(vs) => Ok(Self::Flags(vs)),
            component::Val::Resource(..) => bail!("resources require a store"),
            component::Val::Future(..) => bail!("`future`s require a store"),
            component::Val::Stream(..) => bail!("`stream`s require a store"),
            component::Val::ErrorContext(..) => bail!("`error-context` not supported"),
            component::Val::Map(..) => bail!("maps not supported"),
            component::Val::FixedLengthList(..) => bail!("fixed-length lists not supported"),
        }
    }
}

fn case_type(ty: &component::types::Variant, case: &str) -> Option<Type> {
    ty.cases()
        .find_map(|Case { name, ty }| (name == case).then_some(ty))
        .flatten()
}

fn lift_payload<T: 'static>(
    store: &mut StoreContextMut<'_, T>,
    ty: Option<Type>,
    v: Option<Box<Val>>,
    resource: &mut impl FnMut(&mut T, &Type, Bytes) -> wasmtime::Result<ResourceAny>,
    handles: &mut Vec<component::Val>,
) -> wasmtime::Result<Option<Box<component::Val>>> {
    let Some(v) = v else {
        return Ok(None);
    };
    let ty = ty.context("value type mismatch")?;
    let v = lift_val(store, &ty, *v, resource, handles)?;
    Ok(Some(Box::new(v)))
}

fn lift_val<T: 'static>(
    store: &mut StoreContextMut<'_, T>,
    ty: &Type,
    v: Val,
    resource: &mut impl FnMut(&mut T, &Type, Bytes) -> wasmtime::Result<ResourceAny>,
    handles: &mut Vec<component::Val>,
) -> wasmtime::Result<component::Val> {
    match (v, ty) {
        (Val::List(vs), Type::List(ty)) => {
            let ty = ty.ty();
            let vs = vs
                .into_iter()
                .map(|v| lift_val(store, &ty, v, resource, handles))
                .collect::<wasmtime::Result<_>>()?;
            Ok(component::Val::List(vs))
        }
        (Val::Record(vs), Type::Record(ty)) => {
            let vs = zip(vs, ty.fields())
                .map(|((name, v), Field { ty, .. })| {
                    let v = lift_val(store, &ty, v, resource, handles)?;
                    Ok((name, v))
                })
                .collect::<wasmtime::Result<_>>()?;
            Ok(component::Val::Record(vs))
        }
        (Val::Tuple(vs), Type::Tuple(ty)) => {
            let vs = zip(vs, ty.types())
                .map(|(v, ty)| lift_val(store, &ty, v, resource, handles))
                .collect::<wasmtime::Result<_>>()?;
            Ok(component::Val::Tuple(vs))
        }
        (Val::Variant(case, v), Type::Variant(ty)) => {
            let v = lift_payload(store, case_type(ty, &case), v, resource, handles)?;
            Ok(component::Val::Variant(case, v))
        }
        (Val::Option(v), Type::Option(ty)) => {
            let v = lift_payload(store, Some(ty.ty()), v, resource, handles)?;
            Ok(component::Val::Option(v))
        }
        (Val::Result(Ok(v)), Type::Result(ty)) => {
            let v = lift_payload(store, ty.ok(), v, resource, handles)?;
            Ok(component::Val::Result(Ok(v)))
        }
        (Val::Result(Err(v)), Type::Result(ty)) => {
            let v = lift_payload(store, ty.err(), v, resource, handles)?;
            Ok(component::Val::Result(Err(v)))
        }
        (Val::Resource(v), Type::Own(..) | Type::Borrow(..)) => {
            let v = resource(store.data_mut(), ty, v)?;
            Ok(component::Val::Resource(v))
        }
        #[cfg(feature = "async")]
        (Val::Future(v), ty) => {
            let v = concurrent::lift_future(store.as_context_mut(), ty, v)?;
            handles.push(v.clone());
            Ok(v)
        }
        #[cfg(feature = "async")]
        (Val::Stream(v), ty) => {
            let v = concurrent::lift_stream(store.as_context_mut(), ty, v)?;
            handles.push(v.clone());
            Ok(v)
        }
        #[cfg(not(feature = "async"))]
        (Val::Future(..) | Val::Stream(..), ..) => {
            bail!("`future` and `stream` require the `async` feature")
        }
        (v, ..) => v.try_into(),
    }
}

/// Moves `vals` of types `tys` into `store`.
///
/// `resource` returns the resource of the given `own` or `borrow` type that a handle refers to.
/// `future`s are supported with payloads of primitive types, `string` and none, `stream`s of
/// primitive types and `string`, since Wasmtime requires a static payload type.
///
/// # Errors
///
/// Returns an error if a value cannot be moved, after closing the `future`s and `stream`s
/// created in `store`.
pub fn lift<T: 'static>(
    mut store: impl AsContextMut<Data = T>,
    tys: impl IntoIterator<Item: Borrow<Type>>,
    vals: impl IntoIterator<Item = Val>,
    mut resource: impl FnMut(&mut T, &Type, Bytes) -> wasmtime::Result<ResourceAny>,
) -> wasmtime::Result<Vec<component::Val>> {
    let mut store = store.as_context_mut();
    let mut handles = Vec::default();
    let vals: wasmtime::Result<_> = zip(vals, tys)
        .map(|(v, ty)| lift_val(&mut store, ty.borrow(), v, &mut resource, &mut handles))
        .collect();
    #[cfg(feature = "async")]
    if vals.is_err() {
        concurrent::close(store, handles);
    }
    vals
}

/// Evaluates every item, unlike `collect`
fn collect_all<T>(it: impl IntoIterator<Item = wasmtime::Result<T>>) -> wasmtime::Result<Vec<T>> {
    let vs: Vec<_> = it.into_iter().collect();
    vs.into_iter().collect()
}

fn lower_payload<T: 'static>(
    store: &mut StoreContextMut<'_, T>,
    ty: Option<Type>,
    v: Option<Box<component::Val>>,
    resource: &mut impl FnMut(&mut T, &Type, ResourceAny) -> wasmtime::Result<Bytes>,
) -> wasmtime::Result<Option<Box<Val>>> {
    let Some(v) = v else {
        return Ok(None);
    };
    let ty = ty.context("value type mismatch")?;
    let v = lower_val(store, &ty, *v, resource)?;
    Ok(Some(Box::new(v)))
}

fn lower_val<T: 'static>(
    store: &mut StoreContextMut<'_, T>,
    ty: &Type,
    v: component::Val,
    resource: &mut impl FnMut(&mut T, &Type, ResourceAny) -> wasmtime::Result<Bytes>,
) -> wasmtime::Result<Val> {
    match (v, ty) {
        (component::Val::List(vs), Type::List(ty)) => {
            let ty = ty.ty();
            let vs = collect_all(vs.into_iter().map(|v| lower_val(store, &ty, v, resource)))?;
            Ok(Val::List(vs))
        }
        (component::Val::Record(vs), Type::Record(ty)) => {
            let vs = collect_all(zip(vs, ty.fields()).map(|((name, v), Field { ty, .. })| {
                let v = lower_val(store, &ty, v, resource)?;
                Ok((name, v))
            }))?;
            Ok(Val::Record(vs))
        }
        (component::Val::Tuple(vs), Type::Tuple(ty)) => {
            let vs =
                collect_all(zip(vs, ty.types()).map(|(v, ty)| lower_val(store, &ty, v, resource)))?;
            Ok(Val::Tuple(vs))
        }
        (component::Val::Variant(case, v), Type::Variant(ty)) => {
            let v = lower_payload(store, case_type(ty, &case), v, resource)?;
            Ok(Val::Variant(case, v))
        }
        (component::Val::Option(v), Type::Option(ty)) => {
            let v = lower_payload(store, Some(ty.ty()), v, resource)?;
            Ok(Val::Option(v))
        }
        (component::Val::Result(Ok(v)), Type::Result(ty)) => {
            let v = lower_payload(store, ty.ok(), v, resource)?;
            Ok(Val::Result(Ok(v)))
        }
        (component::Val::Result(Err(v)), Type::Result(ty)) => {
            let v = lower_payload(store, ty.err(), v, resource)?;
            Ok(Val::Result(Err(v)))
        }
        (component::Val::Resource(v), Type::Own(..) | Type::Borrow(..)) => {
            let v = resource(store.data_mut(), ty, v)?;
            Ok(Val::Resource(v))
        }
        #[cfg(feature = "async")]
        (component::Val::Future(mut v), ty) => {
            let v = concurrent::lower_future(store.as_context_mut(), ty, &mut v)?;
            Ok(v)
        }
        #[cfg(feature = "async")]
        (component::Val::Stream(mut v), ty) => {
            let v = concurrent::lower_stream(store.as_context_mut(), ty, &mut v)?;
            Ok(v)
        }
        #[cfg(not(feature = "async"))]
        (component::Val::Future(..) | component::Val::Stream(..), ..) => {
            bail!("`future` and `stream` require the `async` feature")
        }
        (v, ..) => v.try_into(),
    }
}

/// Moves `vals` of types `tys` out of `store`, piping their `future`s and `stream`s.
///
/// `resource` returns the handle of a resource of the given `own` or `borrow` type, which it
/// keeps. `future`s are supported with payloads of primitive types, `string` and none, `stream`s
/// of primitive types and `string`, since Wasmtime requires a static payload type.
///
/// A piped `stream` ends the same way whether the guest closes it or `store` is dropped, so a host
/// dropping `store` before the values are transmitted must fail the transmission itself.
///
/// # Errors
///
/// Returns an error if a value cannot be moved, after closing every `future` and `stream` that
/// was not piped. Resources passed to `resource` stay with it.
pub fn lower<T: 'static>(
    mut store: impl AsContextMut<Data = T>,
    tys: impl IntoIterator<Item: Borrow<Type>>,
    vals: impl IntoIterator<Item = component::Val>,
    mut resource: impl FnMut(&mut T, &Type, ResourceAny) -> wasmtime::Result<Bytes>,
) -> wasmtime::Result<Vec<Val>> {
    let mut store = store.as_context_mut();
    collect_all(zip(vals, tys).map(|(v, ty)| lower_val(&mut store, ty.borrow(), v, &mut resource)))
}
