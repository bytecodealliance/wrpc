use core::iter;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::mem;

use heck::ToUpperCamelCase;
use wit_bindgen_core::wit_parser::{
    Case, Docs, Enum, EnumCase, Field, Flag, Flags, Function, FunctionKind, Handle, Int,
    InterfaceId, Param, Record, Resolve, Result_, Tuple, Type, TypeDefKind, TypeId, TypeOwner,
    Variant, WorldKey,
};
use wit_bindgen_core::{Source, TypeInfo, uwrite, uwriteln};
use wrpc_introspect::{async_paths_ty, is_tuple, is_ty, rpc_func_name};

use crate::{
    Deps, GoWrpc, Identifier, InterfaceName, to_go_ident, to_package_ident, to_upper_camel_case,
};

fn go_func_name(func: &Function) -> String {
    match &func.kind {
        FunctionKind::Constructor(..) => to_upper_camel_case(
            func.name
                .strip_prefix("[constructor]")
                .expect("failed to strip `[constructor]` prefix"),
        ),
        FunctionKind::Static(..) => {
            let name = func
                .name
                .strip_prefix("[static]")
                .expect("failed to strip `[static]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::Method(..) => {
            let name = func
                .name
                .strip_prefix("[method]")
                .expect("failed to strip `[method]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncStatic(..) => {
            let name = func
                .name
                .strip_prefix("[static]")
                .expect("failed to strip `[static]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncMethod(..) => {
            let name = func
                .name
                .strip_prefix("[method]")
                .expect("failed to strip `[method]` prefix");
            let (head, tail) = name.split_once('.').expect("failed to split on `.`");
            format!(
                "{}_{}",
                head.to_upper_camel_case(),
                tail.to_upper_camel_case()
            )
        }
        FunctionKind::AsyncFreestanding => to_upper_camel_case(&func.name),
        FunctionKind::Freestanding => to_upper_camel_case(&func.name),
    }
}

pub fn flatten_ty<'a>(
    resolve: &'a Resolve,
    ty: &Type,
) -> impl Iterator<Item = Type> + 'a + use<'a> {
    let mut ty = *ty;
    loop {
        if let Type::Id(id) = ty {
            match resolve.types[id].kind {
                TypeDefKind::Type(t) => {
                    ty = t;
                    continue;
                }
                TypeDefKind::Tuple(ref t) => {
                    return Box::new(t.types.iter().copied()) as Box<dyn Iterator<Item = _>>;
                }
                _ => {}
            }
        }
        return Box::new(iter::once(ty)) as Box<dyn Iterator<Item = _>>;
    }
}

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) r#gen: &'a mut GoWrpc,
    pub resolve: &'a Resolve,
    pub deps: Deps,
    pub(super) helpers: Source,
    pub(super) helper_names: HashMap<String, String>,
}

impl InterfaceGenerator<'_> {
    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Iterator<Item = &'a Function>,
    ) -> bool {
        let mut methods = vec![];
        let mut funcs_to_export = vec![];

        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }

            funcs_to_export.push(func);
            let prev = mem::take(&mut self.src);
            self.print_docs_and_params(func);
            self.src.push_str(" (");
            for ty in func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
            {
                self.print_opt_ty(&ty, true);
                self.src.push_str(", ");
            }
            self.push_str("error)\n");
            let trait_method = mem::replace(&mut self.src, prev);
            methods.push(trait_method);
        }

        if methods.is_empty() {
            return false;
        }

        uwriteln!(
            self.src,
            "// Handler handles invocations of the exported functions.
//
// The context passed to a method is cancelled once the invocation completes or fails,
// for example, if a pending parameter could not be received. The channel
// of such a parameter is never closed, so a method waiting on it must also
// wait on the context's `Done` channel.
type Handler interface {{"
        );
        for method in &methods {
            self.src.push_str(method);
        }
        let context = self.deps.context();
        let wrpc = self.deps.wrpc();
        let instance = match identifier {
            Identifier::Interface(.., name) => self.resolve.name_canonicalized_world_key(name),
            Identifier::World => String::default(),
        };
        uwriteln!(
            self.src,
            r#"
}}

// HandleInterface serves the invocation with header `hdr` over `c` using `h`.
// It returns false if the function is not exported by this interface.
func HandleInterface(ctx {context}.Context, hdr {wrpc}.Header, c *{wrpc}.Framed, h Handler) (bool, error) {{
    if hdr.Instance != "{instance}" {{
        return false, nil
    }}
    switch hdr.Name {{"#,
        );
        for func in funcs_to_export {
            let name = rpc_func_name(func);
            let (pt, pc) = self.params_tuple(func);
            let (rt, rc, inner) = self.results_tuple(func);
            let p = if func.params.is_empty() { "_" } else { "p" };
            uwrite!(
                self.src,
                r#"
    case "{name}":
        return true, {wrpc}.Serve[{pt}, {rt}](ctx, c, {pc}, {rc}, func(ctx {context}.Context, {p} {pt}) ({rt}, error) {{
            "#,
            );
            let results = func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
                .count();
            for i in 0..results {
                uwrite!(self.src, "r{i}, ");
            }
            uwrite!(self.src, "err := h.{}(ctx", go_func_name(func));
            for i in 0..func.params.len() {
                uwrite!(self.src, ", p.V{i}");
            }
            self.push_str(")\n");
            let rv = match (&func.result, inner) {
                (None, ..) => "struct{}{}".to_string(),
                (Some(..), Some(inner)) => {
                    let members = (0..results)
                        .map(|i| format!("V{i}: r{i}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{rt}{{V0: {inner}{{{members}}}}}")
                }
                (Some(..), None) => format!("{rt}{{V0: r0}}"),
            };
            uwriteln!(
                self.src,
                r#"            return {rv}, err
        }})"#
            );
        }
        self.push_str("}\nreturn false, nil\n}\n");
        true
    }

    pub fn generate_imports<'a>(
        &mut self,
        _identifier: Identifier<'a>,
        instance: &str,
        funcs: impl Iterator<Item = &'a Function>,
    ) {
        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }

            let wrpc = self.deps.wrpc();

            self.print_docs_and_params(func);

            self.src.push_str(" (");
            let results: Box<[Type]> = func
                .result
                .iter()
                .flat_map(|ty| flatten_ty(self.resolve, ty))
                .collect();
            for (i, ty) in results.iter().enumerate() {
                uwrite!(self.src, "r{i}__ ");
                self.print_opt_ty(ty, true);
                self.src.push_str(", ");
            }
            let async_params = func.params.iter().any(|param| self.is_async(&param.ty));
            let async_results = func.result.iter().any(|ty| self.is_async(ty));
            let returns_invocation = func.result.is_some() && (async_params || async_results);
            if returns_invocation {
                uwrite!(self.src, "inv__ *{wrpc}.Invocation, ");
            }
            self.push_str("err__ error) {\n");

            let (pt, pc) = self.params_tuple(func);
            let (rt, rc, inner) = self.results_tuple(func);
            let pv = match func.params.as_slice() {
                [] => "struct{}{}".to_string(),
                params => {
                    let members = params
                        .iter()
                        .enumerate()
                        .map(|(i, Param { name, .. })| format!("V{i}: {}", to_go_ident(name)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{pt}{{{members}}}")
                }
            };
            let res = if results.is_empty() { "_" } else { "res__" };
            let name = rpc_func_name(func);
            uwriteln!(
                self.src,
                r#"{res}, inv__, err__ := {wrpc}.Invoke[{pt}, {rt}](ctx__, c__, "{instance}", "{name}", {pc}, {rc}, {pv})
    if err__ != nil {{
        return
    }}"#,
            );
            if !returns_invocation {
                self.push_str(
                    r"if err__ = inv__.Wait(); err__ != nil {
        return
    }
",
                );
            }
            self.push_str("return ");
            for i in 0..results.len() {
                if inner.is_some() {
                    uwrite!(self.src, "res__.V0.V{i}, ");
                } else {
                    self.push_str("res__.V0, ");
                }
            }
            if returns_invocation {
                self.push_str("inv__, ");
            }
            self.push_str("nil\n}\n");
        }
    }

    /// Returns the type and codec of the parameters of `func`
    fn params_tuple(&mut self, func: &Function) -> (String, String) {
        let types = func.params.iter().map(|Param { ty, .. }| *ty).collect();
        self.root_tuple(types)
    }

    /// Returns the type and codec of the results of `func` and, if the result is a tuple, the type of it
    fn results_tuple(&mut self, func: &Function) -> (String, String, Option<String>) {
        let wrpc = self.deps.wrpc();
        match &func.result {
            None => (
                "struct{}".to_string(),
                format!("{wrpc}.UnitCodec{{}}"),
                None,
            ),
            Some(ty) if is_tuple(self.resolve, ty) => {
                let (t, c) = self.root_tuple(flatten_ty(self.resolve, ty).collect());
                (
                    format!("{wrpc}.Tuple1[{t}]"),
                    format!("{wrpc}.Tuple1Codec[{t}]{{C0: {c}}}"),
                    Some(t),
                )
            }
            Some(ty) => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                (
                    format!("{wrpc}.Tuple1[{t}]"),
                    format!("{wrpc}.Tuple1Codec[{t}]{{C0: {c}}}"),
                    None,
                )
            }
        }
    }

    /// Returns the type and codec of a tuple of `types`, where a single type is a `wrpc.Tuple1`
    fn root_tuple(&mut self, types: Vec<Type>) -> (String, String) {
        let wrpc = self.deps.wrpc();
        match types.as_slice() {
            [] => ("struct{}".to_string(), format!("{wrpc}.UnitCodec{{}}")),
            [ty] => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                (
                    format!("{wrpc}.Tuple1[{t}]"),
                    format!("{wrpc}.Tuple1Codec[{t}]{{C0: {c}}}"),
                )
            }
            _ => {
                let tuple = Tuple { types };
                let t = self.type_string(|g| g.print_tuple(&tuple, true));
                let c = self.tuple_codec(&tuple);
                (t, c)
            }
        }
    }

    fn type_string(&mut self, f: impl FnOnce(&mut Self)) -> String {
        let prev = mem::take(&mut self.src);
        f(self);
        mem::replace(&mut self.src, prev).into()
    }

    fn opt_ty(&mut self, ty: &Type) -> String {
        self.type_string(|g| g.print_opt_ty(ty, true))
    }

    fn value_ty(&mut self, ty: &Type) -> String {
        self.type_string(|g| g.print_ty(ty, true))
    }

    /// Returns the codec of values of type `ty` as printed by `print_opt_ty`
    fn codec(&mut self, ty: &Type) -> String {
        if let Type::Id(id) = ty {
            match &self.resolve.types[*id].kind {
                TypeDefKind::Type(ty) => return self.codec(ty),
                TypeDefKind::Record(..)
                | TypeDefKind::Variant(..)
                | TypeDefKind::Flags(..)
                | TypeDefKind::Result(..)
                | TypeDefKind::Resource => {}
                TypeDefKind::Tuple(Tuple { types }) if types.len() >= 2 => {}
                _ => return self.value_codec(ty),
            }
            let wrpc = self.deps.wrpc();
            let t = self.type_string(|g| g.print_tyid(*id, true));
            let c = self.value_codec(ty);
            return format!("{wrpc}.PtrCodec[{t}]{{Elem: {c}}}");
        }
        self.value_codec(ty)
    }

    /// Returns the codec of values of type `ty` as printed by `print_ty`
    fn value_codec(&mut self, ty: &Type) -> String {
        let wrpc = self.deps.wrpc();
        let id = match ty {
            Type::Bool => return format!("{wrpc}.BoolCodec{{}}"),
            Type::U8 => return format!("{wrpc}.U8Codec{{}}"),
            Type::U16 => return format!("{wrpc}.U16Codec{{}}"),
            Type::U32 => return format!("{wrpc}.U32Codec{{}}"),
            Type::U64 => return format!("{wrpc}.U64Codec{{}}"),
            Type::S8 => return format!("{wrpc}.S8Codec{{}}"),
            Type::S16 => return format!("{wrpc}.S16Codec{{}}"),
            Type::S32 => return format!("{wrpc}.S32Codec{{}}"),
            Type::S64 => return format!("{wrpc}.S64Codec{{}}"),
            Type::F32 => return format!("{wrpc}.F32Codec{{}}"),
            Type::F64 => return format!("{wrpc}.F64Codec{{}}"),
            Type::Char => return format!("{wrpc}.CharCodec{{}}"),
            Type::String => return format!("{wrpc}.StringCodec{{}}"),
            Type::ErrorContext => panic!("error-context is not supported"),
            Type::Id(id) => *id,
        };
        let ty = &self.resolve.types[id];
        match &ty.kind {
            TypeDefKind::Record(..)
            | TypeDefKind::Variant(..)
            | TypeDefKind::Flags(..)
            | TypeDefKind::Enum(..) => {
                let name = ty.name.as_ref().expect("type missing a name");
                let name = format!("{}Codec", to_upper_camel_case(name));
                let name = self.type_path_with_name(id, name);
                format!("{name}{{}}")
            }
            TypeDefKind::Resource => unreachable!(),
            TypeDefKind::Handle(Handle::Own(id)) => {
                let t = self.type_string(|g| g.print_tyid(*id, true));
                format!("{wrpc}.OwnCodec[{t}]{{}}")
            }
            TypeDefKind::Handle(Handle::Borrow(id)) => {
                let t = self.type_string(|g| g.print_tyid(*id, true));
                format!("{wrpc}.BorrowCodec[{t}]{{}}")
            }
            TypeDefKind::Tuple(ty) => self.tuple_codec(ty),
            TypeDefKind::Option(ty) => self.option_codec(ty),
            TypeDefKind::Result(Result_ { ok, err }) => {
                let o = self.type_string(|g| g.print_optional_ty(ok.as_ref()));
                let e = self.type_string(|g| g.print_optional_ty(err.as_ref()));
                let oc = ok.as_ref().map_or_else(
                    || format!("{wrpc}.UnitCodec{{}}"),
                    |ty| self.value_codec(ty),
                );
                let ec = err.as_ref().map_or_else(
                    || format!("{wrpc}.UnitCodec{{}}"),
                    |ty| self.value_codec(ty),
                );
                format!("{wrpc}.ResultCodec[{o}, {e}]{{Ok: {oc}, Err: {ec}}}")
            }
            TypeDefKind::List(ty) if is_ty(self.resolve, Type::U8, ty) => {
                format!("{wrpc}.BytesCodec{{}}")
            }
            TypeDefKind::List(ty) => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                format!("{wrpc}.ListCodec[{t}]{{Elem: {c}}}")
            }
            TypeDefKind::FixedLengthList(ty, size) => {
                let elem = self.opt_ty(ty);
                let t = format!("[{size}]{elem}");
                let c = self.codec(ty);
                self.helper("fixedListCodec", &t, &c, |_| {
                    (
                        format!("return {wrpc}.EncodeFixedList({c}, v[:], dst)\n"),
                        format!(
                            "var v {t}\nrx, err := {wrpc}.DecodeFixedList({c}, r, v[:])\nreturn v, rx, err\n"
                        ),
                    )
                })
            }
            TypeDefKind::Map(key, value) => {
                let kt = self.opt_ty(key);
                let vt = self.opt_ty(value);
                let kc = self.codec(key);
                let vc = self.codec(value);
                format!("{wrpc}.MapCodec[{kt}, {vt}]{{Key: {kc}, Value: {vc}}}")
            }
            TypeDefKind::Future(None) => {
                format!("{wrpc}.FutureCodec[struct{{}}]{{Elem: {wrpc}.UnitCodec{{}}}}")
            }
            TypeDefKind::Future(Some(ty)) => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                format!("{wrpc}.FutureCodec[{t}]{{Elem: {c}}}")
            }
            TypeDefKind::Stream(None) => {
                format!("{wrpc}.StreamCodec[struct{{}}]{{Elem: {wrpc}.UnitCodec{{}}}}")
            }
            TypeDefKind::Stream(Some(ty)) if is_ty(self.resolve, Type::U8, ty) => {
                format!("{wrpc}.ByteStreamCodec{{}}")
            }
            TypeDefKind::Stream(Some(ty)) => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                format!("{wrpc}.StreamCodec[{t}]{{Elem: {c}}}")
            }
            TypeDefKind::Type(ty) => self.value_codec(ty),
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    /// Returns the codec of `option<ty>` values as printed by `print_option`
    fn option_codec(&mut self, ty: &Type) -> String {
        let wrpc = self.deps.wrpc();
        let t = self.value_ty(ty);
        let mut inner = ty;
        while let Type::Id(id) = inner
            && let TypeDefKind::Type(ty) = &self.resolve.types[*id].kind
        {
            inner = ty;
        }
        if let Type::Id(id) = inner
            && let TypeDefKind::List(..)
            | TypeDefKind::Map(..)
            | TypeDefKind::Future(..)
            | TypeDefKind::Stream(..) = self.resolve.types[*id].kind
        {
            let c = self.value_codec(ty);
            return self.helper("optionCodec", &t, &c, |_| {
                (
                    format!(
                        "if v == nil {{\nreturn append(dst, 0), nil, nil\n}}\nreturn ({c}).Encode(v, append(dst, 1))\n"
                    ),
                    format!(
                        "v, rx, err := ({wrpc}.OptionCodec[{t}]{{Elem: {c}}}).Decode(r)\nif err != nil || v == nil {{\nreturn nil, nil, err\n}}\nreturn *v, rx, nil\n"
                    ),
                )
            });
        }
        let c = self.value_codec(ty);
        format!("{wrpc}.OptionCodec[{t}]{{Elem: {c}}}")
    }

    /// Returns the codec of tuple values as printed by `print_tuple`
    fn tuple_codec(&mut self, tuple: &Tuple) -> String {
        let wrpc = self.deps.wrpc();
        match tuple.types.as_slice() {
            [] => format!("{wrpc}.UnitCodec{{}}"),
            [ty] => {
                let t = self.opt_ty(ty);
                let c = self.codec(ty);
                self.helper("tupleCodec", &t, &c, |_| {
                    (
                        format!(
                            "dst, tx, err := ({c}).Encode(v, dst)\nif err != nil {{\nreturn nil, nil, err\n}}\nreturn dst, {wrpc}.NewTransmitters(tx), nil\n"
                        ),
                        format!(
                            "v, rx, err := ({c}).Decode(r)\nif err != nil {{\nreturn v, nil, err\n}}\nreturn v, {wrpc}.NewReceivers(rx), nil\n"
                        ),
                    )
                })
            }
            types if types.len() > 16 => {
                let t = self.type_string(|g| g.print_tuple(tuple, true));
                let cs = types
                    .iter()
                    .map(|ty| self.codec(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.helper("tupleCodec", &t, &cs, |g| {
                    let fields = types
                        .iter()
                        .enumerate()
                        .map(|(i, ty)| (format!("V{i}"), *ty, format!("member {i}")))
                        .collect::<Vec<_>>();
                    g.struct_codec(&t, &fields)
                })
            }
            types => {
                let ts = types
                    .iter()
                    .map(|ty| self.opt_ty(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                let cs = types
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| format!("C{i}: {}", self.codec(ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{wrpc}.Tuple{}Codec[{ts}]{{{cs}}}", types.len())
            }
        }
    }

    /// Returns the bodies of `Encode` and `Decode` of a codec of struct `ty` with `fields`,
    /// which are the field name, type and its description in errors
    fn struct_codec(&mut self, ty: &str, fields: &[(String, Type, String)]) -> (String, String) {
        let wrpc = self.deps.wrpc();
        let fmt = self.deps.fmt();
        let mut enc = String::from("var err error\n");
        let mut dec = format!("var v {ty}\nvar err error\n");
        let mut txs = Vec::with_capacity(fields.len());
        let mut rxs = Vec::with_capacity(fields.len());
        for (i, (field, ty, desc)) in fields.iter().enumerate() {
            let c = self.codec(ty);
            let (tx, rx) = if self.is_async(ty) {
                uwriteln!(enc, "var tx{i} {wrpc}.Transmitter");
                uwriteln!(dec, "var rx{i} {wrpc}.Receiver");
                txs.push(format!("tx{i}"));
                rxs.push(format!("rx{i}"));
                (format!("tx{i}"), format!("rx{i}"))
            } else {
                txs.push("nil".to_string());
                rxs.push("nil".to_string());
                ("_".to_string(), "_".to_string())
            };
            uwriteln!(
                enc,
                r#"if dst, {tx}, err = ({c}).Encode(v.{field}, dst); err != nil {{
    return nil, nil, {fmt}.Errorf("failed to encode {desc}: %w", err)
}}"#
            );
            uwriteln!(
                dec,
                r#"if v.{field}, {rx}, err = ({c}).Decode(r); err != nil {{
    return v, nil, {fmt}.Errorf("failed to decode {desc}: %w", err)
}}"#
            );
        }
        if fields.iter().any(|(_, ty, _)| self.is_async(ty)) {
            uwriteln!(
                enc,
                "return dst, {wrpc}.NewTransmitters({}), nil",
                txs.join(", ")
            );
            uwriteln!(
                dec,
                "return v, {wrpc}.NewReceivers({}), nil",
                rxs.join(", ")
            );
        } else {
            enc.push_str("return dst, nil, nil\n");
            dec.push_str("return v, nil, nil\n");
        }
        (enc, dec)
    }

    /// Returns the definition of codec type `name` of values of type `ty` with the bodies of
    /// its `Encode` and `Decode` methods
    fn codec_type(&mut self, name: &str, ty: &str, (enc, dec): (String, String)) -> String {
        let wrpc = self.deps.wrpc();
        format!(
            r#"
type {name} struct{{}}

func ({name}) Encode(v {ty}, dst []byte) ([]byte, {wrpc}.Transmitter, error) {{
{enc}}}

func ({name}) Decode(r {wrpc}.ByteReader) ({ty}, {wrpc}.Receiver, error) {{
{dec}}}
"#
        )
    }

    /// Returns a codec of values of type `ty` built from the codecs `inner`, defining it using `f`
    /// unless it is already defined
    fn helper(
        &mut self,
        prefix: &str,
        ty: &str,
        inner: &str,
        f: impl FnOnce(&mut Self) -> (String, String),
    ) -> String {
        let key = format!("{prefix} {ty} {inner}");
        if let Some(name) = self.helper_names.get(&key) {
            return format!("{name}{{}}");
        }
        let name = format!("{prefix}{}", self.r#gen.helper_count);
        self.r#gen.helper_count += 1;
        self.helper_names.insert(key, name.clone());
        let bodies = f(self);
        let def = self.codec_type(&name, ty, bodies);
        self.helpers.push_str(&def);
        format!("{name}{{}}")
    }

    pub fn finish(&mut self) -> String {
        let mut src: String = mem::take(&mut self.src).into();
        src.push_str(&mem::take(&mut self.helpers));
        src
    }

    pub fn start_append_submodule(&mut self, name: &WorldKey) -> (String, Vec<String>) {
        let snake = match name {
            WorldKey::Name(name) => to_package_ident(name),
            WorldKey::Interface(id) => {
                to_package_ident(self.resolve.interfaces[*id].name.as_ref().unwrap())
            }
        };
        let module_path = crate::compute_module_path(name, self.resolve, !self.in_import);
        (snake, module_path)
    }

    pub fn finish_append_submodule(mut self, snake: &str, module_path: Vec<String>) {
        let module = self.finish();
        let module = format!(
            r#"package {snake}

{}

{module}"#,
            self.deps,
        );
        let map = if self.in_import {
            &mut self.r#gen.import_modules
        } else {
            &mut self.r#gen.export_modules
        };
        map.push((module, module_path));
    }

    fn godoc(&mut self, docs: &Docs) {
        let docs = match &docs.contents {
            Some(docs) => docs,
            None => return,
        };
        for line in docs.trim().lines() {
            self.push_str("//");
            if !line.is_empty() {
                self.push_str(" ");
                self.push_str(line);
            }
            self.push_str("\n");
        }
    }

    fn godoc_params(&mut self, docs: &[Param], header: &str) {
        let _ = (docs, header);
        // let docs = docs
        //     .iter()
        //     .filter(|param| param.docs.trim().len() > 0)
        //     .collect::<Vec<_>>();
        // if docs.len() == 0 {
        //     return;
        // }

        // self.push_str("///\n");
        // self.push_str("/// ## ");
        // self.push_str(header);
        // self.push_str("\n");
        // self.push_str("///\n");

        // for param in docs {
        //     for (i, line) in param.docs.lines().enumerate() {
        //         self.push_str("/// ");
        //         // Currently wasi only has at most one return value, so there's no
        //         // need to indent it or name it.
        //         if header != "Return" {
        //             if i == 0 {
        //                 self.push_str("* `");
        //                 self.push_str(to_go_ident(param.name.as_str()));
        //                 self.push_str("` - ");
        //             } else {
        //                 self.push_str("  ");
        //             }
        //         }
        //         self.push_str(line);
        //         self.push_str("\n");
        //     }
        // }
    }

    fn print_docs_and_params(&mut self, func: &Function) {
        self.godoc(&func.docs);
        self.godoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.godoc_params(&func.results, "Return");

        if self.in_import {
            self.push_str("func ");
        }
        if self.in_import && matches!(func.kind, FunctionKind::Constructor(..)) {
            self.push_str("New");
        }
        self.push_str(&go_func_name(func));
        let context = self.deps.context();
        uwrite!(self.src, "(ctx__ {context}.Context, ");
        if self.in_import {
            let wrpc = self.deps.wrpc();
            uwrite!(self.src, "c__ *{wrpc}.Framed, ");
        }
        for Param {
            name, ty: param, ..
        } in &func.params
        {
            self.push_str(&to_go_ident(name));
            self.push_str(" ");
            self.print_opt_ty(param, true);
            self.push_str(",");
        }
        self.push_str(")");
    }

    fn print_ty(&mut self, ty: &Type, decl: bool) {
        match ty {
            Type::Id(t) => self.print_tyid(*t, decl),
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("uint8"),
            Type::U16 => self.push_str("uint16"),
            Type::U32 => self.push_str("uint32"),
            Type::U64 => self.push_str("uint64"),
            Type::S8 => self.push_str("int8"),
            Type::S16 => self.push_str("int16"),
            Type::S32 => self.push_str("int32"),
            Type::S64 => self.push_str("int64"),
            Type::F32 => self.push_str("float32"),
            Type::F64 => self.push_str("float64"),
            Type::Char => self.push_str("rune"),
            Type::String => self.push_str("string"),
            Type::ErrorContext => self.push_str("struct{}"),
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>) {
        match ty {
            Some(ty) => self.print_ty(ty, true),
            None => self.push_str("struct{}"),
        }
    }

    fn type_path_with_name(&mut self, id: TypeId, name: String) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner
            && let Some(path) = self.path_to_interface(id)
        {
            return format!("{path}.{name}");
        }
        name
    }

    fn is_async(&self, ty: &Type) -> bool {
        let (paths, fut) = async_paths_ty(self.resolve, ty);
        fut || !paths.is_empty()
    }

    fn print_opt_ty(&mut self, ty: &Type, decl: bool) {
        match ty {
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Handle(..) => self.print_tyid(*id, decl),
                    TypeDefKind::Tuple(ty) if ty.types.len() < 2 => self.print_tuple(ty, decl),
                    TypeDefKind::Enum(..) => {
                        let name = ty.name.as_ref().expect("enum missing a name");
                        let name = self.type_path_with_name(*id, to_upper_camel_case(name));
                        self.push_str(&name);
                    }
                    TypeDefKind::Option(ty) => self.print_option(ty, decl),
                    TypeDefKind::List(ty) => self.print_list(ty),
                    TypeDefKind::FixedLengthList(ty, size) => {
                        self.print_fixed_length_list(ty, *size)
                    }
                    TypeDefKind::Map(key, value) => self.print_map(key, value),
                    TypeDefKind::Future(ty) => self.print_future(ty),
                    TypeDefKind::Stream(ty) => self.print_stream(ty),
                    TypeDefKind::Type(ty) => self.print_opt_ty(ty, decl),
                    _ => {
                        if decl {
                            self.push_str("*");
                        } else {
                            self.push_str("&");
                        }
                        self.print_tyid(*id, true);
                    }
                }
            }
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("uint8"),
            Type::U16 => self.push_str("uint16"),
            Type::U32 => self.push_str("uint32"),
            Type::U64 => self.push_str("uint64"),
            Type::S8 => self.push_str("int8"),
            Type::S16 => self.push_str("int16"),
            Type::S32 => self.push_str("int32"),
            Type::S64 => self.push_str("int64"),
            Type::F32 => self.push_str("float32"),
            Type::F64 => self.push_str("float64"),
            Type::Char => self.push_str("rune"),
            Type::String => self.push_str("string"),
            Type::ErrorContext => self.push_str("struct{}"),
        }
    }

    fn print_option(&mut self, ty: &Type, decl: bool) {
        let mut inner = ty;
        while let Type::Id(id) = inner
            && let TypeDefKind::Type(ty) = &self.resolve.types[*id].kind
        {
            inner = ty;
        }
        if let Type::Id(id) = inner
            && let TypeDefKind::List(..)
            | TypeDefKind::Map(..)
            | TypeDefKind::Future(..)
            | TypeDefKind::Stream(..) = self.resolve.types[*id].kind
        {
            // Go slices, maps and channels are nillable
            self.print_ty(ty, true);
            return;
        }
        if decl {
            self.push_str("*");
        } else {
            self.push_str("&");
        }
        self.print_ty(ty, true);
    }

    fn print_result(&mut self, ty: &Result_) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Result[");
        self.print_optional_ty(ty.ok.as_ref());
        self.push_str(",");
        self.print_optional_ty(ty.err.as_ref());
        self.push_str("]");
    }

    fn print_tuple(&mut self, Tuple { types }: &Tuple, decl: bool) {
        match types.as_slice() {
            [] => self.push_str("struct{}"),
            [ty] => self.print_opt_ty(ty, decl),
            _ if types.len() > 16 => {
                self.push_str("struct {");
                for (i, ty) in types.iter().enumerate() {
                    uwrite!(self.src, "V{i} ");
                    self.print_opt_ty(ty, true);
                    self.push_str("; ");
                }
                self.push_str("}");
            }
            _ => {
                let wrpc = self.deps.wrpc();
                self.push_str(wrpc);
                self.push_str(".Tuple");
                uwrite!(self.src, "{}[", types.len());
                for ty in types {
                    self.print_opt_ty(ty, true);
                    self.push_str(",");
                }
                self.push_str("]");
            }
        }
    }

    fn print_list(&mut self, ty: &Type) {
        self.push_str("[]");
        self.print_opt_ty(ty, true);
    }

    fn print_fixed_length_list(&mut self, ty: &Type, size: u32) {
        uwrite!(self.src, "[{size}]");
        self.print_opt_ty(ty, true);
    }

    fn print_map(&mut self, key: &Type, value: &Type) {
        self.push_str("map[");
        self.print_opt_ty(key, true);
        self.push_str("]");
        self.print_opt_ty(value, true);
    }

    fn print_future(&mut self, ty: &Option<Type>) {
        self.push_str("<-chan ");
        if let Some(ty) = ty {
            self.print_opt_ty(ty, true);
        } else {
            self.push_str("struct{}");
        }
    }

    fn print_stream(&mut self, element: &Option<Type>) {
        self.push_str("<-chan ");
        match element {
            Some(ty) => self.print_list(ty),
            None => self.push_str("[]struct{}"),
        }
    }

    fn print_own(&mut self, id: TypeId) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Own[");
        self.print_tyid(id, true);
        self.push_str("]");
    }

    fn print_borrow(&mut self, id: TypeId) {
        let wrpc = self.deps.wrpc();
        self.push_str(wrpc);
        self.push_str(".Borrow[");
        self.print_tyid(id, true);
        self.push_str("]");
    }

    fn print_tyid(&mut self, id: TypeId, decl: bool) {
        let ty = &self.resolve.types[id];
        if let Some(name) = &ty.name {
            let name = self.type_path_with_name(id, to_upper_camel_case(name));
            self.push_str(&name);
            return;
        }
        match &ty.kind {
            TypeDefKind::List(ty) => self.print_list(ty),
            TypeDefKind::FixedLengthList(ty, size) => self.print_fixed_length_list(ty, *size),
            TypeDefKind::Map(key, value) => self.print_map(key, value),
            TypeDefKind::Option(ty) => self.print_option(ty, decl),
            TypeDefKind::Result(ty) => self.print_result(ty),
            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),
            TypeDefKind::Tuple(ty) => self.print_tuple(ty, decl),
            TypeDefKind::Resource => panic!("unsupported anonymous type reference: resource"),
            TypeDefKind::Record(_) => panic!("unsupported anonymous type reference: record"),
            TypeDefKind::Flags(_) => panic!("unsupported anonymous type reference: flags"),
            TypeDefKind::Enum(_) => panic!("unsupported anonymous type reference: enum"),
            TypeDefKind::Future(ty) => self.print_future(ty),
            TypeDefKind::Stream(ty) => self.print_stream(ty),
            TypeDefKind::Handle(Handle::Own(id)) => self.print_own(*id),
            TypeDefKind::Handle(Handle::Borrow(id)) => self.print_borrow(*id),
            TypeDefKind::Type(t) => self.print_ty(t, decl),
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn int_repr(&mut self, repr: Int) {
        match repr {
            Int::U8 => self.push_str("uint8"),
            Int::U16 => self.push_str("uint16"),
            Int::U32 => self.push_str("uint32"),
            Int::U64 => self.push_str("uint64"),
        }
    }

    fn name_of(&self, ty: TypeId) -> Option<String> {
        let info = self.info(ty);

        // If this type isn't actually used, no need to generate it.
        (info.owned || info.borrowed)
            .then(|| to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap()))
    }

    fn path_to_interface(&mut self, interface: InterfaceId) -> Option<String> {
        let InterfaceName {
            import_name,
            import_path,
            ..
        } = &self.r#gen.interface_names[&interface];
        if let Identifier::Interface(cur, _) = self.identifier
            && cur == interface
        {
            return None;
        }
        Some(self.deps.import(import_name.clone(), import_path.clone()))
    }

    fn push_str(&mut self, s: &str) {
        self.src.push_str(s);
    }

    fn info(&self, ty: TypeId) -> TypeInfo {
        self.r#gen.types.get(ty)
    }
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(
        &mut self,
        id: TypeId,
        _name: &str,
        Record { fields, .. }: &Record,
        docs: &Docs,
    ) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwriteln!(self.src, "type {name} struct {{");
            for Field { name, ty, docs, .. } in fields {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" ");
                self.print_opt_ty(ty, true);
                self.push_str("\n");
            }
            self.push_str("}\n");

            // TODO: Print something more useful
            uwriteln!(
                self.src,
                r#"func (v *{name}) String() string {{ return "{name}" }}"#
            );
            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }

            let fields = fields
                .iter()
                .map(|Field { name, ty, .. }| {
                    (name.to_upper_camel_case(), *ty, format!("field `{name}`"))
                })
                .collect::<Vec<_>>();
            let bodies = self.struct_codec(&name, &fields);
            let def = self.codec_type(&format!("{name}Codec"), &name, bodies);
            self.push_str(&def);
        }
    }

    fn type_resource(&mut self, _id: TypeId, name: &str, docs: &Docs) {
        self.godoc(docs);
        uwriteln!(
            self.src,
            "type {} interface {{}}",
            to_upper_camel_case(name)
        );
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_tuple(tuple, true);
            self.push_str("\n");
        }
    }

    fn type_flags(&mut self, id: TypeId, _name: &str, ty: &Flags, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            let strings = self.deps.strings();

            // Struct
            self.godoc(docs);
            uwriteln!(self.src, "type {name} struct {{");
            for Flag { name, docs, .. } in &ty.flags {
                self.godoc(docs);
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" bool\n");
            }
            self.push_str("}\n");

            // String()
            uwriteln!(self.src, "func (v *{name}) String() string {{");
            uwriteln!(self.src, "flags := make([]string, 0, {})", ty.flags.len());
            for Flag { name, .. } in &ty.flags {
                self.push_str("if v.");
                self.push_str(&name.to_upper_camel_case());
                self.push_str(" {\n");
                uwriteln!(self.src, r#"flags = append(flags, "{name}")"#);
                self.push_str("}\n");
            }
            uwriteln!(self.src, r#"return {strings}.Join(flags, " | ")"#);
            self.push_str("}\n\n");

            // Error()
            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }

            let wrpc = self.deps.wrpc();
            let n = ty.flags.len();
            if n > 64 {
                panic!("unsupported number of flags: {n}");
            }
            let mut enc = String::from("var bits uint64\n");
            let mut dec = String::default();
            if n == 0 {
                uwriteln!(
                    dec,
                    "if _, _, err := ({wrpc}.FlagsCodec{{N: 0}}).Decode(r); err != nil {{\nreturn {name}{{}}, nil, err\n}}\nreturn {name}{{}}, nil, nil"
                );
            } else {
                uwriteln!(
                    dec,
                    "bits, _, err := ({wrpc}.FlagsCodec{{N: {n}}}).Decode(r)\nif err != nil {{\nreturn {name}{{}}, nil, err\n}}\nreturn {name}{{"
                );
            }
            for (i, Flag { name, .. }) in ty.flags.iter().enumerate() {
                let field = name.to_upper_camel_case();
                uwriteln!(enc, "if v.{field} {{\nbits |= 1 << {i}\n}}");
                uwriteln!(dec, "{field}: bits&(1<<{i}) != 0,");
            }
            uwriteln!(
                enc,
                "return ({wrpc}.FlagsCodec{{N: {n}}}).Encode(bits, dst)"
            );
            if n > 0 {
                dec.push_str("}, nil, nil\n");
            }
            let def = self.codec_type(&format!("{name}Codec"), &name, (enc, dec));
            self.push_str(&def);
        }
    }

    fn type_variant(&mut self, id: TypeId, _name: &str, variant: &Variant, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwriteln!(
                self.src,
                r#"type {name} struct {{ payload any; discriminant {name}Discriminant }}"#
            );
            uwriteln!(
                self.src,
                r#"func (v *{name}) Discriminant() {name}Discriminant {{ return v.discriminant }}"#
            );
            uwrite!(self.src, r#"type {name}Discriminant "#);
            self.int_repr(variant.tag());
            self.push_str("\n");
            self.push_str("const (\n");
            for (
                i,
                Case {
                    name: case_name,
                    docs,
                    ..
                },
            ) in variant.cases.iter().enumerate()
            {
                self.godoc(docs);
                self.push_str(&name);
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(" ");
                self.push_str(&name);
                uwriteln!(self.src, "Discriminant = {i}");
            }
            self.push_str(")\n");

            uwriteln!(
                self.src,
                r#"func (v *{name}) String() string {{ switch v.discriminant {{"#
            );
            for Case {
                name: case_name, ..
            } in &variant.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(": return \"");
                self.push_str(case_name);
                self.push_str("\"\n");
            }
            self.push_str("default: panic(\"invalid variant\")\n}\n");
            self.push_str("}\n");

            for Case {
                name: case_name,
                ty,
                docs,
                ..
            } in &variant.cases
            {
                let camel = case_name.to_upper_camel_case();
                self.godoc(docs);
                uwrite!(self.src, r#"func (v *{name}) Get{camel}() ("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                    self.push_str(", ");
                }
                self.push_str("ok bool) {\n");
                uwriteln!(
                    self.src,
                    r#"if ok = (v.discriminant == {name}{camel}); !ok {{ return }}"#
                );
                if let Some(ty) = ty {
                    self.push_str("payload, ok = v.payload.(");
                    self.print_opt_ty(ty, true);
                    self.push_str(")\n");
                }
                self.push_str("return\n}\n");

                self.godoc(docs);
                uwrite!(self.src, r#"func (v *{name}) Set{camel}("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                }
                uwriteln!(self.src, ") *{name} {{");
                uwriteln!(self.src, "v.discriminant = {name}{camel}");
                if ty.is_some() {
                    self.push_str("v.payload = payload\n");
                } else {
                    self.push_str("v.payload = nil\n");
                }
                self.push_str("return v\n}\n");

                self.godoc(docs);
                uwrite!(self.src, r#"func New{name}{camel}("#);
                if let Some(ty) = ty {
                    self.push_str("payload ");
                    self.print_opt_ty(ty, true);
                }
                uwriteln!(self.src, ") *{name} {{");
                uwriteln!(self.src, "return (&{name}{{}}).Set{camel}(");
                if ty.is_some() {
                    self.push_str("payload");
                }
                self.push_str(")\n}\n");
            }

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v *{name}) Error() string {{ return v.String() }}"#
                );
            }

            let errors = self.deps.errors();
            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            let mut enc = format!(
                "dst = {wrpc}.AppendUint32(dst, uint32(v.discriminant))\nswitch v.discriminant {{\n"
            );
            let mut dec = format!(
                "var v {name}\nn, err := {wrpc}.ReadUint32(r)\nif err != nil {{\nreturn v, nil, err\n}}\nswitch n {{\n"
            );
            for (
                i,
                Case {
                    name: case_name,
                    ty,
                    ..
                },
            ) in variant.cases.iter().enumerate()
            {
                let camel = case_name.to_upper_camel_case();
                uwriteln!(enc, "case {name}{camel}:");
                uwriteln!(dec, "case {i}:");
                if let Some(ty) = ty {
                    let t = self.opt_ty(ty);
                    let c = self.codec(ty);
                    uwriteln!(
                        enc,
                        r#"payload, ok := v.payload.({t})
if !ok {{
    return nil, nil, {errors}.New("invalid `{case_name}` payload")
}}
return ({c}).Encode(payload, dst)"#
                    );
                    uwriteln!(
                        dec,
                        r#"payload, rx, err := ({c}).Decode(r)
if err != nil {{
    return v, nil, {fmt}.Errorf("failed to decode `{case_name}` payload: %w", err)
}}
v.Set{camel}(payload)
return v, rx, nil"#
                    );
                } else {
                    enc.push_str("return dst, nil, nil\n");
                    uwriteln!(dec, "v.Set{camel}()\nreturn v, nil, nil");
                }
            }
            uwriteln!(
                enc,
                r#"default:
    return nil, nil, {errors}.New("invalid variant discriminant")
}}"#
            );
            uwriteln!(
                dec,
                r#"default:
    return v, nil, {fmt}.Errorf("unknown discriminant value %d", n)
}}"#
            );
            let def = self.codec_type(&format!("{name}Codec"), &name, (enc, dec));
            self.push_str(&def);
        }
    }

    fn type_option(&mut self, id: TypeId, _name: &str, payload: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_option(payload, true);
            self.push_str("\n");
        }
    }

    fn type_result(&mut self, id: TypeId, _name: &str, result: &Result_, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_result(result);
            self.push_str("\n");
        }
    }

    fn type_enum(&mut self, id: TypeId, _name: &str, enum_: &Enum, docs: &Docs) {
        let info = self.info(id);
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, r#"type {name} "#);
            self.int_repr(enum_.tag());
            self.push_str("\n");
            self.push_str("const (\n");
            for (
                i,
                EnumCase {
                    name: case_name,
                    docs,
                    ..
                },
            ) in enum_.cases.iter().enumerate()
            {
                self.godoc(docs);
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(" ");
                self.push_str(&name);
                uwriteln!(self.src, " = {i}");
            }
            self.push_str(")\n");

            uwriteln!(
                self.src,
                r#"func (v {name}) String() string {{ switch v {{"#
            );
            for EnumCase {
                name: case_name, ..
            } in &enum_.cases
            {
                self.push_str("case ");
                self.push_str(&name);
                self.push_str("_");
                self.push_str(&case_name.to_upper_camel_case());
                self.push_str(": return \"");
                self.push_str(case_name);
                self.push_str("\"\n");
            }
            self.push_str("default: panic(\"invalid enum\")\n}\n");
            self.push_str("}\n");

            if info.error {
                uwriteln!(
                    self.src,
                    r#"func (v {name}) Error() string {{ return v.String() }}"#
                );
            }

            let errors = self.deps.errors();
            let fmt = self.deps.fmt();
            let wrpc = self.deps.wrpc();
            let n = enum_.cases.len();
            let def = self.codec_type(
                &format!("{name}Codec"),
                &name,
                (
                    format!(
                        r#"if uint64(v) >= {n} {{
    return nil, nil, {errors}.New("invalid enum value")
}}
return {wrpc}.AppendUint32(dst, uint32(v)), nil, nil
"#
                    ),
                    format!(
                        r#"n, err := {wrpc}.ReadUint32(r)
if err != nil {{
    return 0, nil, err
}}
if uint64(n) >= {n} {{
    return 0, nil, {fmt}.Errorf("unknown discriminant value %d", n)
}}
return {name}(n), nil, nil
"#
                    ),
                ),
            );
            self.push_str(&def);
        }
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_ty(ty, true);
            self.push_str("\n");
        }
    }

    fn type_list(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_list(ty);
            self.push_str("\n");
        }
    }

    fn type_fixed_length_list(
        &mut self,
        id: TypeId,
        _name: &str,
        ty: &Type,
        size: u32,
        docs: &Docs,
    ) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_fixed_length_list(ty, size);
            self.push_str("\n");
        }
    }

    fn type_map(&mut self, id: TypeId, _name: &str, key: &Type, value: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_map(key, value);
            self.push_str("\n");
        }
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.godoc(docs);
        uwrite!(self.src, "type {} = ", name.to_upper_camel_case());
        self.print_ty(ty, true);
        self.src.push_str("\n");
    }

    fn type_future(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_future(ty);
            self.push_str("\n");
        }
    }

    fn type_stream(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.godoc(docs);
            uwrite!(self.src, "type {name} = ");
            self.print_stream(ty);
            self.push_str("\n");
        }
    }
}
