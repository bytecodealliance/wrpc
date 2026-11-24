use std::collections::BTreeSet;

use wit_parser::{
    Case, Field, Function, FunctionKind, Handle, Record, Resolve, Result_, Type, TypeDefKind,
    TypeId, TypeIdVisitor, WorldId,
};

#[must_use]
pub fn rpc_func_name(func: &Function) -> &str {
    match &func.kind {
        FunctionKind::Constructor(..) => func
            .name
            .strip_prefix("[constructor]")
            .expect("failed to strip `[constructor]` prefix"),
        FunctionKind::Static(..) => func
            .name
            .strip_prefix("[static]")
            .expect("failed to strip `[static]` prefix"),
        FunctionKind::Method(..) => func
            .name
            .strip_prefix("[method]")
            .expect("failed to strip `[method]` prefix"),
        FunctionKind::AsyncStatic(..) => func
            .name
            .strip_prefix("[static]")
            .expect("failed to strip `[static]` prefix"),
        FunctionKind::AsyncMethod(..) => func
            .name
            .strip_prefix("[method]")
            .expect("failed to strip `[method]` prefix"),
        FunctionKind::AsyncFreestanding => &func.name,
        FunctionKind::Freestanding => &func.name,
    }
}

#[must_use]
pub fn is_ty(resolve: &Resolve, expected: Type, ty: &Type) -> bool {
    let mut ty = *ty;
    loop {
        if ty == expected {
            return true;
        }
        if let Type::Id(id) = ty
            && let TypeDefKind::Type(t) = resolve.types[id].kind
        {
            ty = t;
            continue;
        }
        return false;
    }
}

#[must_use]
pub fn is_list_of(resolve: &Resolve, expected: Type, ty: &Type) -> bool {
    let mut ty = *ty;
    loop {
        if let Type::Id(id) = ty {
            match resolve.types[id].kind {
                TypeDefKind::Type(t) => {
                    ty = t;
                    continue;
                }
                TypeDefKind::List(t) => return is_ty(resolve, expected, &t),
                _ => return false,
            }
        }
        return false;
    }
}

#[must_use]
pub fn is_tuple(resolve: &Resolve, ty: &Type) -> bool {
    let mut ty = *ty;
    loop {
        if let Type::Id(id) = ty {
            match resolve.types[id].kind {
                TypeDefKind::Type(t) => {
                    ty = t;
                    continue;
                }
                TypeDefKind::Tuple(_) => return true,
                _ => return false,
            }
        }
        return false;
    }
}

/// Whether `ty` contains a `future` or a `stream`
#[must_use]
pub fn is_async_ty(resolve: &Resolve, ty: &Type) -> bool {
    if let Type::Id(id) = ty {
        is_async_tyid(resolve, *id)
    } else {
        false
    }
}

/// Whether the type `id` contains a `future` or a `stream`
#[must_use]
pub fn is_async_tyid(resolve: &Resolve, id: TypeId) -> bool {
    match &resolve.types[id].kind {
        TypeDefKind::List(ty)
        | TypeDefKind::FixedLengthList(ty, _)
        | TypeDefKind::Map(_, ty)
        | TypeDefKind::Option(ty)
        | TypeDefKind::Type(ty) => is_async_ty(resolve, ty),
        TypeDefKind::Result(Result_ { ok, err }) => {
            ok.iter().chain(err).any(|ty| is_async_ty(resolve, ty))
        }
        TypeDefKind::Variant(ty) => ty
            .cases
            .iter()
            .filter_map(|Case { ty, .. }| ty.as_ref())
            .any(|ty| is_async_ty(resolve, ty)),
        TypeDefKind::Tuple(ty) => ty.types.iter().any(|ty| is_async_ty(resolve, ty)),
        TypeDefKind::Record(Record { fields }) => fields
            .iter()
            .any(|Field { ty, .. }| is_async_ty(resolve, ty)),
        TypeDefKind::Future(..) | TypeDefKind::Stream(..) => true,
        TypeDefKind::Resource
        | TypeDefKind::Flags(..)
        | TypeDefKind::Enum(..)
        | TypeDefKind::Handle(Handle::Own(..) | Handle::Borrow(..)) => false,
        TypeDefKind::Unknown => unreachable!(),
    }
}

/// Whether `world` uses `error-context` anywhere
#[must_use]
pub fn uses_error_context(resolve: &Resolve, world: WorldId) -> bool {
    #[derive(Default)]
    struct Visitor {
        visited: BTreeSet<TypeId>,
        found: bool,
    }

    impl TypeIdVisitor for Visitor {
        fn before_visit_type_id(&mut self, id: TypeId) -> bool {
            self.visited.insert(id)
        }

        fn visit_type(&mut self, resolve: &Resolve, ty: &Type) {
            match ty {
                Type::Id(id) => self.visit_type_id(resolve, *id),
                Type::ErrorContext => self.found = true,
                _ => {}
            }
        }
    }

    let mut visitor = Visitor::default();
    visitor.visit_world(resolve, world);
    visitor.found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_async() {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_source(
                "test.wit",
                "package test:test;
                interface i {
                    type m = map<string, tuple<u8, future<map<u32, stream<u8>>>>>;
                    type n = map<string, tuple<u8, list<u32>>>;
                }",
            )
            .unwrap();
        let (_, iface) = resolve.packages[pkg].interfaces.first().unwrap();
        let types = &resolve.interfaces[*iface].types;
        assert!(is_async_tyid(&resolve, types["m"]));
        assert!(!is_async_tyid(&resolve, types["n"]));
    }
}
