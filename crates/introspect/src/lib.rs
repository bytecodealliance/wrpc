use std::collections::{BTreeSet, VecDeque};

use wit_parser::{
    Case, Field, Flags, Function, FunctionKind, Handle, Int, Record, Resolve, Stream, Type,
    TypeDefKind, TypeId,
};

#[must_use]
pub fn flag_repr(ty: &Flags) -> Int {
    match ty.flags.len() {
        ..=8 => Int::U8,
        9..=16 => Int::U16,
        17..=32 => Int::U32,
        33.. => Int::U64,
    }
}

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
        FunctionKind::Method(..) => func.item_name(),
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
        if let Type::Id(id) = ty {
            if let TypeDefKind::Type(t) = resolve.types[id].kind {
                ty = t;
                continue;
            }
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
pub fn async_paths_ty(resolve: &Resolve, ty: &Type) -> (BTreeSet<VecDeque<Option<u32>>>, bool) {
    if let Type::Id(ty) = ty {
        async_paths_tyid(resolve, *ty)
    } else {
        (BTreeSet::default(), false)
    }
}

#[must_use]
pub fn async_paths_tyid(resolve: &Resolve, id: TypeId) -> (BTreeSet<VecDeque<Option<u32>>>, bool) {
    match &resolve.types[id].kind {
        TypeDefKind::List(ty) => {
            let mut paths = BTreeSet::default();
            let (nested, fut) = async_paths_ty(resolve, ty);
            for mut path in nested {
                path.push_front(None);
                paths.insert(path);
            }
            if fut {
                paths.insert(vec![None].into());
            }
            (paths, false)
        }
        TypeDefKind::Option(ty) => async_paths_ty(resolve, ty),
        TypeDefKind::Result(ty) => {
            let mut paths = BTreeSet::default();
            let mut is_fut = false;
            if let Some(ty) = ty.ok.as_ref() {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for path in nested {
                    paths.insert(path);
                }
                if fut {
                    is_fut = true;
                }
            }
            if let Some(ty) = ty.err.as_ref() {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for path in nested {
                    paths.insert(path);
                }
                if fut {
                    is_fut = true;
                }
            }
            (paths, is_fut)
        }
        TypeDefKind::Variant(ty) => {
            let mut paths = BTreeSet::default();
            let mut is_fut = false;
            for Case { ty, .. } in &ty.cases {
                if let Some(ty) = ty {
                    let (nested, fut) = async_paths_ty(resolve, ty);
                    for path in nested {
                        paths.insert(path);
                    }
                    if fut {
                        is_fut = true;
                    }
                }
            }
            (paths, is_fut)
        }
        TypeDefKind::Tuple(ty) => {
            let mut paths = BTreeSet::default();
            for (i, ty) in ty.types.iter().enumerate() {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for mut path in nested {
                    path.push_front(Some(i.try_into().unwrap()));
                    paths.insert(path);
                }
                if fut {
                    let path = vec![Some(i.try_into().unwrap())].into();
                    paths.insert(path);
                }
            }
            (paths, false)
        }
        TypeDefKind::Record(Record { fields }) => {
            let mut paths = BTreeSet::default();
            for (i, Field { ty, .. }) in fields.iter().enumerate() {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for mut path in nested {
                    path.push_front(Some(i.try_into().unwrap()));
                    paths.insert(path);
                }
                if fut {
                    let path = vec![Some(i.try_into().unwrap())].into();
                    paths.insert(path);
                }
            }
            (paths, false)
        }
        TypeDefKind::Future(ty) => {
            let mut paths = BTreeSet::default();
            if let Some(ty) = ty {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for mut path in nested {
                    path.push_front(Some(0));
                    paths.insert(path);
                }
                if fut {
                    paths.insert(vec![Some(0)].into());
                }
            }
            (paths, true)
        }
        TypeDefKind::Stream(Stream { element, .. }) => {
            let mut paths = BTreeSet::new();
            if let Some(ty) = element {
                let (nested, fut) = async_paths_ty(resolve, ty);
                for mut path in nested {
                    path.push_front(None);
                    paths.insert(path);
                }
                if fut {
                    paths.insert(vec![None].into());
                }
            }
            (paths.into_iter().collect(), true)
        }
        TypeDefKind::Type(ty) => async_paths_ty(resolve, ty),
        TypeDefKind::Resource
        | TypeDefKind::Flags(..)
        | TypeDefKind::Enum(..)
        | TypeDefKind::Handle(Handle::Own(..) | Handle::Borrow(..)) => (BTreeSet::default(), false),
        TypeDefKind::Unknown => unreachable!(),
    }
}
