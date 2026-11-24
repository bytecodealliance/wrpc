use crate::{
    FnSig, Identifier, InterfaceName, RustFlagsRepr, RustWrpc, TypeGeneration, full_wit_type_name,
    int_repr, to_rust_ident, to_upper_camel_case,
};
use heck::{ToKebabCase, ToShoutySnakeCase, ToUpperCamelCase};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::mem;
use wit_bindgen_core::wit_parser::{
    Case, Docs, Enum, Field, Flags, Function, FunctionKind, Handle, Int, InterfaceId, Param,
    Record, Resolve, Result_, Tuple, Type, TypeDefKind, TypeId, TypeOwner, Variant, WorldKey,
};
use wit_bindgen_core::{Source, TypeInfo, dealias, uwrite, uwriteln};
use wrpc_introspect::{async_paths_ty, async_paths_tyid, is_ty, rpc_func_name};

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) r#gen: &'a mut RustWrpc,
    pub resolve: &'a Resolve,
    pub(super) decoding: bool,
    pub(super) encoding: bool,
}

impl InterfaceGenerator<'_> {
    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Iterator<Item = &'a Function> + Clone,
    ) -> bool {
        let mut traits = BTreeMap::new();
        let mut funcs_to_export = Vec::new();

        traits.insert(None, ("Handler".to_string(), Vec::new()));

        if let Identifier::Interface(id, ..) = identifier {
            for (name, id) in &self.resolve.interfaces[id].types {
                match self.resolve.types[*id].kind {
                    TypeDefKind::Resource => {}
                    _ => continue,
                }
                let camel = name.to_upper_camel_case();
                traits.insert(Some(*id), (format!("Handler{camel}"), Vec::new()));
            }
        }

        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }

            let resource = match func.kind {
                FunctionKind::Freestanding | FunctionKind::AsyncFreestanding => None,
                FunctionKind::Method(id)
                | FunctionKind::Constructor(id)
                | FunctionKind::Static(id)
                | FunctionKind::AsyncMethod(id)
                | FunctionKind::AsyncStatic(id) => Some(id),
            };
            funcs_to_export.push(func);
            let (_, methods) = traits.get_mut(&resource).unwrap();

            let prev = mem::take(&mut self.src);
            self.print_export_sig(func);
            self.src.push_str(";\n");
            let trait_method = mem::replace(&mut self.src, prev);
            methods.push(trait_method);
        }

        let (name, methods) = traits.remove(&None).unwrap();
        if !methods.is_empty() || !traits.is_empty() {
            let resource_traits: Vec<_> = traits.values().map(|(name, _)| name.as_str()).collect();
            let resource_traits = resource_traits.join(" + ");

            uwriteln!(self.src, "pub trait {name}: {resource_traits} {{");
            for method in methods {
                self.src.push_str(&method);
            }
            uwriteln!(self.src, "}}");
        }

        for (trait_name, methods) in traits.values() {
            uwriteln!(self.src, "pub trait {trait_name}: 'static {{");
            for method in methods {
                self.src.push_str(method);
            }
            uwriteln!(self.src, "}}");
        }

        if funcs_to_export.is_empty() {
            return false;
        }

        let trait_names: Vec<_> = traits
            .values()
            .map(|(name, ..)| format!(" + {name}"))
            .collect();

        let instance = match identifier {
            Identifier::Interface(.., name) => self.resolve.name_canonicalized_world_key(name),
            Identifier::World => String::new(),
        };
        uwrite!(
            self.src,
            r#"
pub fn handle_interface<S__, H__>(
    handler: &H__,
    instance: &str,
    func: &str,
    srv: S__,
) -> ::core::result::Result<
    impl ::core::future::Future<Output = ::core::result::Result<S__::Output, {wrpc_core}::ServeError>> + ::core::marker::Send + use<S__, H__>,
    S__,
>
where
    S__: {wrpc_core}::Serve + ::core::marker::Send,
    H__: Handler{resource_traits} + ::core::marker::Send + ::core::marker::Sync + ::core::clone::Clone,
{{
    let ::core::option::Option::Some(func) = lookup__(instance, func) else {{
        return ::core::result::Result::Err(srv);
    }};
    ::core::result::Result::Ok(serve__(::core::clone::Clone::clone(handler), func, srv))
}}

pub(crate) fn lookup__(instance: &str, func: &str) -> ::core::option::Option<usize> {{
    match (instance, func) {{"#,
            resource_traits = trait_names.join(""),
            wrpc_core = self.r#gen.wrpc_core_path(),
        );
        for (i, func) in funcs_to_export.iter().enumerate() {
            uwrite!(
                self.src,
                r#"
        ("{instance}", "{}") => ::core::option::Option::Some({i}),"#,
                rpc_func_name(func),
            );
        }
        uwrite!(
            self.src,
            r#"
        _ => ::core::option::Option::None,
    }}
}}

pub(crate) async fn serve__<S__, H__>(
    handler: H__,
    func: usize,
    srv: S__,
) -> ::core::result::Result<S__::Output, {wrpc_core}::ServeError>
where
    S__: {wrpc_core}::Serve,
    H__: Handler{resource_traits} + ::core::marker::Send + ::core::marker::Sync,
{{
    match func {{"#,
            resource_traits = trait_names.join(""),
            wrpc_core = self.r#gen.wrpc_core_path(),
        );
        for (i, func) in funcs_to_export.iter().enumerate() {
            let params_ty = func
                .params
                .iter()
                .map(|param| format!("{}, ", self.decoded_ty(&param.ty)))
                .collect::<String>();
            let params_tuple = self.use_tuple(func.params.len(), false);
            let params_ty = match &params_tuple {
                Some(tuple) => format!("{tuple}<{params_ty}>"),
                None => format!("({params_ty})"),
            };
            let results_ty = func
                .result
                .as_ref()
                .map(|ty| format!("{}, ", self.encoded_ty(ty)))
                .unwrap_or_default();
            uwrite!(
                self.src,
                r#"
        {i} => {{
            let res = {wrpc_core}::serve::<{params_ty}, ({results_ty}), _, _>(srv, move |{params_tuple}("#,
                wrpc_core = self.r#gen.wrpc_core_path(),
                params_tuple = params_tuple.unwrap_or_default(),
            );
            for i in 0..func.params.len() {
                uwrite!(self.src, "p{i}, ");
            }
            self.push_str(")| async move {");
            for (i, param) in func.params.iter().enumerate() {
                if self.has_raw_strings(&param.ty) {
                    let ty = self.decoded_ty(&param.ty);
                    uwrite!(self.src, "let p{i}: {ty} = p{i};");
                }
            }
            let (trait_name, name) = match func.kind {
                FunctionKind::Freestanding | FunctionKind::AsyncFreestanding => {
                    ("Handler", to_rust_ident(func.item_name()).to_string())
                }
                FunctionKind::Method(id)
                | FunctionKind::Constructor(id)
                | FunctionKind::Static(id)
                | FunctionKind::AsyncMethod(id)
                | FunctionKind::AsyncStatic(id) => (
                    traits
                        .get(&Some(id))
                        .map(|(name, _)| name.as_str())
                        .unwrap(),
                    if let FunctionKind::Constructor(_) = func.kind {
                        "new".to_string()
                    } else {
                        to_rust_ident(func.item_name()).to_string()
                    },
                ),
            };
            uwrite!(
                self.src,
                r#"
                    {tracing}::trace!(instance = "{instance}", func = "{wit_name}", "calling handler");
                    let results = {trait_name}::{name}(&handler"#,
                tracing = self.r#gen.tracing_path(),
                wit_name = func.name,
            );
            for (i, param) in func.params.iter().enumerate() {
                let p = self.decoded_value(&param.ty, &format!("p{i}"), false);
                uwrite!(self.src, ", {p}");
            }
            self.push_str(").await?;\n");
            let results = if let Some(ty) = &func.result {
                // wrap single-element results into a tuple for correct indexing
                format!("({},)", self.encoded_value(ty, "results", false, false))
            } else {
                "results".to_string()
            };
            uwrite!(
                self.src,
                r#"
                    ::core::result::Result::Ok::<_, ::std::io::Error>({results})
            }});
            res.await
        }}"#,
            );
        }

        uwriteln!(
            self.src,
            r#"
        _ => ::core::unreachable!(),
    }}
}}"#,
        );
        true
    }

    fn print_export_sig(&mut self, func: &Function) {
        let mut sig = FnSig {
            use_item_name: true,
            private: true,
            ..Default::default()
        };
        sig.self_arg = Some("&self".into());
        sig.self_is_first_param = false;

        self.print_docs_and_params(func, &sig);
        match func.result.iter().len() {
            0 => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = ::std::io::Result<()>> + ::core::marker::Send",
                );
            }
            1 => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = ::std::io::Result<",
                );
                let ty = func.result.iter().next().unwrap();
                self.print_ty(ty, true, false);
                self.push_str(">> + ::core::marker::Send");
            }
            _ => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = ::std::io::Result<(",
                );
                for ty in func.result.iter() {
                    self.print_ty(ty, true, false);
                    self.push_str(", ");
                }
                self.push_str(")>> + ::core::marker::Send");
            }
        }
    }

    pub(super) fn generate_stub<'a>(
        &mut self,
        interface: Option<InterfaceId>,
        funcs: impl Iterator<Item = &'a Function>,
    ) {
        let path = match interface {
            Some(id) => format!("{}::", self.path_to_interface(id).unwrap()),
            None => String::default(),
        };
        let mut traits = BTreeMap::default();
        traits.insert(None, (format!("{path}Handler"), String::default()));
        if let Some(id) = interface {
            for (name, id) in &self.resolve.interfaces[id].types {
                if let TypeDefKind::Resource = self.resolve.types[*id].kind {
                    let camel = name.to_upper_camel_case();
                    traits.insert(
                        Some(*id),
                        (format!("{path}Handler{camel}"), String::default()),
                    );
                }
            }
        }

        for func in funcs {
            if self.r#gen.skip.contains(&func.name) {
                continue;
            }
            let prev = mem::take(&mut self.src);
            self.src.push_str("#[allow(unused_variables)]\n");
            self.print_export_sig(func);
            self.src.push_str(" { async { ::core::unreachable!() } }\n");
            let method = mem::replace(&mut self.src, prev);
            let (.., methods) = traits.get_mut(&func.kind.resource()).unwrap();
            methods.push_str(&method);
        }

        let (name, methods) = traits.remove(&None).unwrap();
        if !methods.is_empty() || !traits.is_empty() {
            uwriteln!(self.src, "impl {name} for Stub {{\n{methods}}}");
        }
        for (name, methods) in traits.values() {
            uwriteln!(self.src, "impl {name} for Stub {{\n{methods}}}");
        }
    }

    pub fn generate_imports<'a>(
        &mut self,
        instance: &str,
        funcs: impl Iterator<Item = &'a Function>,
    ) {
        for func in funcs {
            self.generate_guest_import(instance, func);
        }
    }

    pub fn finish(&mut self) -> String {
        mem::take(&mut self.src).into()
    }

    fn path_to_root(&self) -> String {
        let mut path_to_root = String::default();

        if let Identifier::Interface(_, key) = self.identifier {
            // Escape the submodule for this interface
            path_to_root.push_str("super::");

            // Escape the `exports` top-level submodule
            if !self.in_import {
                path_to_root.push_str("super::");
            }

            // Escape the namespace/package submodules for interface-based ids
            match key {
                WorldKey::Name(_) => {}
                WorldKey::Interface(_) => {
                    path_to_root.push_str("super::super::");
                }
            }
        }
        path_to_root
    }

    pub fn start_append_submodule(&mut self, name: &WorldKey) -> (String, Vec<String>) {
        let snake = match name {
            WorldKey::Name(name) => to_rust_ident(name),
            WorldKey::Interface(id) => {
                to_rust_ident(self.resolve.interfaces[*id].name.as_ref().unwrap())
            }
        };
        let module_path = crate::compute_module_path(name, self.resolve, !self.in_import);
        (snake, module_path)
    }

    pub fn finish_append_submodule(mut self, snake: &str, module_path: Vec<String>, docs: &Docs) {
        let module = self.finish();

        self.rustdoc(docs);
        let docs = mem::take(&mut self.src).to_string();
        let docs = docs.trim_end();

        let module = format!(
            "\
                {docs}
                #[allow(dead_code, clippy::all)]
                pub mod {snake} {{
                    {module}
                }}
",
        );
        let map = if self.in_import {
            &mut self.r#gen.import_modules
        } else {
            &mut self.r#gen.export_modules
        };
        map.push((module, module_path));
    }

    fn generate_guest_import(&mut self, instance: &str, func: &Function) {
        if self.r#gen.skip.contains(&func.name) {
            return;
        }

        let fqn = if instance.is_empty() {
            func.name.to_string()
        } else {
            format!("{instance}.{}", func.name)
        };

        let mut sig = FnSig::default();
        match func.kind {
            FunctionKind::Method(id) | FunctionKind::AsyncMethod(id) => {
                let name = self.resolve.types[id].name.as_ref().unwrap();
                let name = to_upper_camel_case(name);
                uwriteln!(self.src, "impl {name} {{");

                sig.use_item_name = true;
                sig.self_arg = Some("&self".into());
                sig.self_is_first_param = true;
            }
            FunctionKind::Static(id)
            | FunctionKind::Constructor(id)
            | FunctionKind::AsyncStatic(id) => {
                let name = self.resolve.types[id].name.as_ref().unwrap();
                let name = to_upper_camel_case(name);
                uwriteln!(self.src, "impl {name} {{");

                sig.use_item_name = true;
            }
            FunctionKind::Freestanding | FunctionKind::AsyncFreestanding => {}
        }
        self.src.push_str("#[allow(clippy::all)]\n");

        let async_params = func.params.iter().any(|param| self.is_async(&param.ty));
        let async_results = func.result.iter().any(|ty| self.is_async(ty));

        let anyhow = self.r#gen.anyhow_path();

        let params = self.print_docs_and_params(func, &sig);
        match func.result.iter().collect::<Vec<_>>().as_slice() {
            [] => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = {anyhow}::Result<()>> + use<'a, C__>"
                );
            }
            [ty] => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = {anyhow}::Result<"
                );
                if async_params || async_results {
                    self.push_str("(");
                }
                self.print_ty(ty, true, false);
                if async_params || async_results {
                    uwrite!(
                        self.src,
                        ", impl ::core::future::Future<Output = ::std::io::Result<()>> + use<C__>)",
                    );
                }
                uwrite!(self.src, ">> + use<'a, C__>");
            }
            types => {
                uwrite!(
                    self.src,
                    " -> impl ::core::future::Future<Output = {anyhow}::Result<(",
                );
                for ty in types {
                    self.print_ty(ty, true, false);
                    self.push_str(", ");
                }
                if async_params || async_results {
                    uwrite!(
                        self.src,
                        "impl ::core::future::Future<Output = ::std::io::Result<()>> + use<C__>",
                    );
                }
                uwrite!(self.src, ")>> + use<'a, C__>");
            }
        };
        self.push_str("{\n");

        let decoded = func
            .result
            .as_ref()
            .filter(|ty| self.has_raw_strings(ty))
            .map(|ty| self.decoded_ty(ty));
        let result = func
            .result
            .as_ref()
            .map(|ty| self.decoded_value(ty, "wrpc__", false));
        let args_tuple = self.use_tuple(func.params.len(), false);
        let args_ty = self.import_args_ty(func);
        let results_ty = func
            .result
            .as_ref()
            .map(|ty| format!("{}, ", self.decoded_ty(ty)))
            .unwrap_or_default();
        let wrpc_core = self.r#gen.wrpc_core_path();
        let params = {
            let s = params.join(", ");
            if params.len() == 1 {
                format!("{s},")
            } else {
                s
            }
        };
        let params = format!("{}({params})", args_tuple.unwrap_or_default());
        let name = rpc_func_name(func);
        let call = format!(
            r#"{wrpc_core}::invoke::<{args_ty}, ({results_ty}), _>(clt__, "{instance}", "{name}", {params})"#
        );
        let pat = match &result {
            Some(..) => format!(
                "((wrpc__,), io__){}",
                decoded.map_or_else(String::default, |ty| format!(": (({ty},), _)"))
            ),
            None => "((), io__)".to_string(),
        };
        let (io, ret) = match result {
            Some(result) if async_params || async_results => {
                (String::default(), format!("({result}, io__)"))
            }
            result => (
                format!(r#"{anyhow}::Context::context(io__.await, "failed to invoke `{fqn}`")?;"#),
                result.unwrap_or_else(|| "()".to_string()),
            ),
        };
        uwriteln!(
            self.src,
            r#"
                async move {{
                    let {pat} = {anyhow}::Context::context({call}.await, "failed to invoke `{fqn}`")?;
                    {io}
                    Ok({ret})
                }}
            }}"#,
        );

        match func.kind {
            FunctionKind::Freestanding | FunctionKind::AsyncFreestanding => {}
            FunctionKind::Method(_)
            | FunctionKind::Static(_)
            | FunctionKind::Constructor(_)
            | FunctionKind::AsyncMethod(_)
            | FunctionKind::AsyncStatic(_) => {
                self.src.push_str("}\n");
            }
        }
    }

    fn rustdoc(&mut self, docs: &Docs) {
        let docs = match &docs.contents {
            Some(docs) => docs,
            None => return,
        };
        for line in docs.trim().lines() {
            self.push_str("///");
            if !line.is_empty() {
                self.push_str(" ");
                self.push_str(line);
            }
            self.push_str("\n");
        }
    }

    fn rustdoc_params(&mut self, docs: &[Param], header: &str) {
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
        //                 self.push_str(to_rust_ident(param.name.as_str()));
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

    fn print_docs_and_params(&mut self, func: &Function, sig: &FnSig) -> Vec<String> {
        self.rustdoc(&func.docs);
        self.rustdoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.rustdoc_params(&func.results, "Return");

        if !sig.private {
            self.push_str("pub ");
        }
        self.push_str("fn ");
        if sig.use_item_name {
            if let FunctionKind::Constructor(_) = &func.kind {
                self.push_str("new");
            } else {
                self.push_str(&to_rust_ident(func.item_name()));
            }
        } else {
            self.push_str(&to_rust_ident(func.item_name()));
        };
        if self.in_import {
            let args_ty = self.import_args_ty(func);
            uwrite!(
                self.src,
                "<'a, C__: {wrpc_core}::Invoke<<{args_ty} as {wrpc_core}::Encode>::Encoder>>(clt__: C__,",
                wrpc_core = self.r#gen.wrpc_core_path(),
            );
        } else {
            self.push_str("(");
            if let Some(arg) = &sig.self_arg {
                self.push_str(arg);
                self.push_str(",");
            }
        }
        let mut params = Vec::new();
        for (
            i,
            Param {
                name, ty: param, ..
            },
        ) in func.params.iter().enumerate()
        {
            if i == 0 && sig.self_is_first_param && !self.in_import {
                continue;
            }

            let name = to_rust_ident(name);
            self.push_str(&name);
            self.push_str(": ");

            if self.in_import {
                self.print_import_param_ty(param);
            } else {
                self.print_ty(param, true, false);
            }
            params.push(name);
            self.push_str(",");
        }
        self.push_str(")");
        params
    }

    fn print_ty(&mut self, ty: &Type, owned: bool, submodule: bool) {
        // If we have a typedef of a string or a list and it is being borrowed,
        // the typedef is an alias for `String` or `Vec<T>`; borrow it as `&str`
        // or `&[T]` so callers don't need to create owned copies.
        if !owned && let Type::Id(id) = ty {
            let id = dealias(self.resolve, *id);
            match &self.resolve.types[id].kind {
                TypeDefKind::Type(Type::String) => {
                    self.print_borrowed_str();
                    return;
                }
                TypeDefKind::List(element) => {
                    let element = *element;
                    self.print_list(&element, false, submodule);
                    return;
                }
                TypeDefKind::Map(key, value) => {
                    let (key, value) = (*key, *value);
                    self.print_map(&key, &value, false, submodule);
                    return;
                }
                _ => {}
            }
        }

        match ty {
            Type::Id(t) => self.print_tyid(*t, owned, submodule),
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("u8"),
            Type::U16 => self.push_str("u16"),
            Type::U32 => self.push_str("u32"),
            Type::U64 => self.push_str("u64"),
            Type::S8 => self.push_str("i8"),
            Type::S16 => self.push_str("i16"),
            Type::S32 => self.push_str("i32"),
            Type::S64 => self.push_str("i64"),
            Type::F32 => self.push_str("f32"),
            Type::F64 => self.push_str("f64"),
            Type::Char => self.push_str("char"),
            Type::String => {
                if !owned {
                    self.print_borrowed_str();
                } else if self.r#gen.opts.raw_strings && self.encoding {
                    uwrite!(
                        self.src,
                        "{}::Utf8Bytes<Vec<u8>>",
                        self.r#gen.wrpc_core_path()
                    );
                } else if self.r#gen.opts.raw_strings && !self.decoding {
                    self.push_str("Vec<u8>");
                } else {
                    self.push_str("String");
                }
            }
            Type::ErrorContext => self.push_str("()"),
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>, owned: bool, submodule: bool) {
        match ty {
            Some(ty) => {
                self.print_ty(ty, owned, submodule);
            }
            None => self.push_str("()"),
        }
    }

    fn type_path_with_name(&self, id: TypeId, name: String, submodule: bool) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner
            && let Some(path) = self.path_to_interface(id)
        {
            if submodule {
                return format!("super::{path}::{name}");
            } else {
                return format!("{path}::{name}");
            }
        }
        if submodule {
            format!("super::{name}")
        } else {
            name
        }
    }

    fn print_import_param_ty(&mut self, ty: &Type) {
        match ty {
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Tuple(ty) => self.print_tuple(ty, false, false),
                    TypeDefKind::Enum(..) => {
                        let name = ty.name.as_ref().expect("enum missing a name");
                        let name = self.type_path_with_name(*id, to_upper_camel_case(name), false);
                        self.push_str(&name);
                    }
                    TypeDefKind::Option(ty) => self.print_option(ty, false, false),
                    TypeDefKind::List(ty) => self.print_list(ty, false, false),
                    TypeDefKind::Future(ty) => self.print_future(ty, false),
                    TypeDefKind::Stream(ty) => self.print_stream(ty, false),
                    TypeDefKind::Type(ty) => self.print_import_param_ty(ty),
                    _ => {
                        let (paths, fut) = async_paths_tyid(self.resolve, *id);
                        if !fut && paths.is_empty() {
                            self.push_str("&'a ");
                        }
                        self.print_tyid(*id, true, false);
                    }
                }
            }
            Type::Bool => self.push_str("bool"),
            Type::U8 => self.push_str("u8"),
            Type::U16 => self.push_str("u16"),
            Type::U32 => self.push_str("u32"),
            Type::U64 => self.push_str("u64"),
            Type::S8 => self.push_str("i8"),
            Type::S16 => self.push_str("i16"),
            Type::S32 => self.push_str("i32"),
            Type::S64 => self.push_str("i64"),
            Type::F32 => self.push_str("f32"),
            Type::F64 => self.push_str("f64"),
            Type::Char => self.push_str("char"),
            Type::String => self.print_borrowed_str(),
            Type::ErrorContext => self.push_str("()"),
        }
    }

    fn print_borrowed_str(&mut self) {
        if self.r#gen.opts.raw_strings {
            self.push_str("&'a [u8]");
        } else {
            self.push_str("&'a str");
        }
    }

    fn has_raw_strings(&self, ty: &Type) -> bool {
        if !self.r#gen.opts.raw_strings {
            return false;
        }
        let Type::Id(id) = ty else {
            return matches!(ty, Type::String);
        };
        let ty = &self.resolve.types[*id];
        if ty.name.is_some() {
            if let Some(TypeGeneration::Remap(..)) =
                self.r#gen.with.get(&full_wit_type_name(self.resolve, *id))
            {
                return false;
            }
            if let TypeOwner::Interface(id) = ty.owner
                && self.r#gen.interface_names[&id].remapped
            {
                return false;
            }
        }
        match &ty.kind {
            TypeDefKind::Type(ty)
            | TypeDefKind::List(ty)
            | TypeDefKind::FixedLengthList(ty, ..)
            | TypeDefKind::Option(ty) => self.has_raw_strings(ty),
            TypeDefKind::Result(Result_ { ok, err }) => {
                ok.iter().chain(err).any(|ty| self.has_raw_strings(ty))
            }
            TypeDefKind::Map(key, value) => {
                self.has_raw_strings(key) || self.has_raw_strings(value)
            }
            TypeDefKind::Tuple(Tuple { types }) => types.iter().any(|ty| self.has_raw_strings(ty)),
            TypeDefKind::Future(ty) | TypeDefKind::Stream(ty) => {
                ty.as_ref().is_some_and(|ty| self.has_raw_strings(ty))
            }
            _ => false,
        }
    }

    fn print_decoded_ty(&mut self, ty: &Type, submodule: bool) {
        let decoding = mem::replace(&mut self.decoding, true);
        self.print_ty(ty, true, submodule);
        self.decoding = decoding;
    }

    fn decoded_ty(&mut self, ty: &Type) -> String {
        let prev = mem::take(&mut self.src);
        self.print_decoded_ty(ty, false);
        mem::replace(&mut self.src, prev).to_string()
    }

    fn is_async(&self, ty: &Type) -> bool {
        let (paths, fut) = async_paths_ty(self.resolve, ty);
        fut || !paths.is_empty()
    }

    fn encoded_ty(&mut self, ty: &Type) -> String {
        let prev = mem::take(&mut self.src);
        self.print_encoded_ty(ty, false);
        mem::replace(&mut self.src, prev).to_string()
    }

    fn import_args_ty(&mut self, func: &Function) -> String {
        let args_ty = func
            .params
            .iter()
            .map(|param| format!("{}, ", self.import_param_ty(&param.ty)))
            .collect::<String>();
        match self.use_tuple(func.params.len(), false) {
            Some(tuple) => format!("{tuple}<{args_ty}>"),
            None => format!("({args_ty})"),
        }
    }

    fn import_param_ty(&mut self, ty: &Type) -> String {
        let prev = mem::take(&mut self.src);
        self.print_import_param_ty(ty);
        mem::replace(&mut self.src, prev).to_string()
    }

    fn print_encoded_ty(&mut self, ty: &Type, submodule: bool) {
        let encoding = mem::replace(&mut self.encoding, true);
        self.print_ty(ty, true, submodule);
        self.encoding = encoding;
    }

    fn decoded_value(&self, ty: &Type, v: &str, submodule: bool) -> String {
        self.map_raw_strings(ty, v, false, submodule, &|v| format!("{v}.into_bytes()"))
    }

    fn encoded_value(&self, ty: &Type, v: &str, by_ref: bool, submodule: bool) -> String {
        let wrpc_core = self.r#gen.wrpc_core_path();
        self.map_raw_strings(ty, v, by_ref, submodule, &|v| {
            format!("{wrpc_core}::Utf8Bytes({v})")
        })
    }

    fn map_raw_strings(
        &self,
        ty: &Type,
        v: &str,
        by_ref: bool,
        submodule: bool,
        string: &dyn Fn(&str) -> String,
    ) -> String {
        if !self.has_raw_strings(ty) {
            return v.to_string();
        }
        let Type::Id(id) = ty else {
            return string(v);
        };
        let futures = self.r#gen.futures_path();
        match &self.resolve.types[*id].kind {
            TypeDefKind::Type(ty) => self.map_raw_strings(ty, v, by_ref, submodule, string),
            TypeDefKind::List(ty) => format!(
                "{v}.{}(|v| {}).collect::<Vec<_>>()",
                if by_ref {
                    "iter().map"
                } else {
                    "into_iter().map"
                },
                self.map_raw_strings(ty, "v", by_ref, submodule, string)
            ),
            TypeDefKind::FixedLengthList(ty, ..) => format!(
                "{v}.{}(|v| {})",
                if by_ref { "each_ref().map" } else { "map" },
                self.map_raw_strings(ty, "v", by_ref, submodule, string)
            ),
            TypeDefKind::Map(key, value) => format!(
                "{v}.{}(|(k, v)| ({}, {})).collect::<{}<_, _>>()",
                if by_ref {
                    "iter().map"
                } else {
                    "into_iter().map"
                },
                self.map_raw_strings(key, "k", by_ref, submodule, string),
                self.map_raw_strings(value, "v", by_ref, submodule, string),
                self.r#gen.map_type_path(),
            ),
            TypeDefKind::Option(ty) => format!(
                "{v}.{}(|v| {})",
                if by_ref { "as_ref().map" } else { "map" },
                self.map_raw_strings(ty, "v", by_ref, submodule, string)
            ),
            TypeDefKind::Result(Result_ { ok, err }) => {
                let ok = ok.as_ref().map_or("v".into(), |ty| {
                    self.map_raw_strings(ty, "v", by_ref, submodule, string)
                });
                let err = err.as_ref().map_or("v".into(), |ty| {
                    self.map_raw_strings(ty, "v", by_ref, submodule, string)
                });
                format!(
                    "{v}.{}(|v| {ok}).map_err(|v| {err})",
                    if by_ref { "as_ref().map" } else { "map" }
                )
            }
            TypeDefKind::Tuple(Tuple { types }) => {
                let (names, values): (Vec<_>, Vec<_>) = types
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| {
                        let name = format!("v{i}");
                        let value = self.map_raw_strings(ty, &name, by_ref, submodule, string);
                        (name, value)
                    })
                    .unzip();
                let tuple = self.tuple_path(types.len(), submodule).unwrap_or_default();
                format!(
                    "{{ let {tuple}({},) = {v}; {tuple}({},) }}",
                    names.join(", "),
                    values.join(", ")
                )
            }
            TypeDefKind::Future(Some(ty)) => format!(
                "::std::boxed::Box::pin({futures}::FutureExt::map({v}, |v| {})) as ::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = _> + ::core::marker::Send>>",
                self.map_raw_strings(ty, "v", by_ref, submodule, string)
            ),
            TypeDefKind::Stream(Some(ty)) => format!(
                "::std::boxed::Box::pin({futures}::StreamExt::map({v}, |v| v.into_iter().map(|v| {}).collect::<Vec<_>>())) as ::core::pin::Pin<::std::boxed::Box<dyn {futures}::Stream<Item = _> + ::core::marker::Send>>",
                self.map_raw_strings(ty, "v", by_ref, submodule, string)
            ),
            _ => unreachable!(),
        }
    }

    fn tuple_path(&self, n: usize, submodule: bool) -> Option<String> {
        (n > 12).then(|| {
            let sup = if submodule { "super::" } else { "" };
            format!("{sup}{}Tuple{n}", self.path_to_root())
        })
    }

    fn use_tuple(&mut self, n: usize, submodule: bool) -> Option<String> {
        let path = self.tuple_path(n, submodule)?;
        self.r#gen.tuples.insert(n);
        Some(path)
    }

    fn print_tuple(&mut self, Tuple { types }: &Tuple, owned: bool, submodule: bool) {
        let (open, close) = match self.use_tuple(types.len(), submodule) {
            Some(path) => (format!("{path}<"), ">"),
            None => ("(".to_string(), ")"),
        };
        self.push_str(&open);
        for ty in types {
            self.print_ty(ty, owned, submodule);
            self.push_str(",");
        }
        self.push_str(close);
    }

    fn print_option(&mut self, ty: &Type, owned: bool, submodule: bool) {
        self.push_str("::core::option::Option<");
        self.print_ty(ty, owned, submodule);
        self.push_str(">");
    }

    fn print_result(&mut self, ty: &Result_, owned: bool, submodule: bool) {
        self.push_str("::core::result::Result<");
        self.print_optional_ty(ty.ok.as_ref(), owned, submodule);
        self.push_str(",");
        self.print_optional_ty(ty.err.as_ref(), owned, submodule);
        self.push_str(">");
    }

    fn print_list(&mut self, ty: &Type, owned: bool, submodule: bool) {
        if owned {
            if is_ty(self.resolve, Type::U8, ty) {
                uwrite!(self.src, "{bytes}::Bytes", bytes = self.r#gen.bytes_path());
            } else {
                self.push_str("Vec<");
                self.print_ty(ty, true, submodule);
                self.push_str(">");
            }
        } else if is_ty(self.resolve, Type::U8, ty) {
            uwrite!(
                self.src,
                "&'a {bytes}::Bytes",
                bytes = self.r#gen.bytes_path()
            );
        } else {
            self.push_str("&'a [");
            self.print_ty(ty, false, submodule);
            self.push_str("]");
        }
    }

    fn print_map(&mut self, key: &Type, value: &Type, owned: bool, submodule: bool) {
        if !owned {
            self.push_str("&'a ");
        }
        uwrite!(self.src, "{}<", self.r#gen.map_type_path());
        self.print_ty(key, true, submodule);
        self.push_str(", ");
        self.print_ty(value, true, submodule);
        self.push_str(">");
    }

    fn print_fixed_length_list(&mut self, ty: &Type, size: u32, owned: bool, submodule: bool) {
        if !owned {
            self.push_str("&'a ");
        }
        self.push_str("[");
        self.print_ty(ty, true, submodule);
        uwrite!(self.src, "; {size}]");
    }

    fn print_future(&mut self, ty: &Option<Type>, submodule: bool) {
        self.push_str("::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = ");
        if let Some(ty) = ty {
            self.print_ty(ty, true, submodule);
        } else {
            self.push_str("()");
        }
        self.push_str("> + ::core::marker::Send>>");
    }

    fn print_stream(&mut self, element: &Option<Type>, submodule: bool) {
        uwrite!(
            self.src,
            "::core::pin::Pin<::std::boxed::Box<dyn {futures}::Stream<Item = ",
            futures = self.r#gen.futures_path()
        );
        if let Some(ty) = element {
            self.print_list(ty, true, submodule);
        } else {
            self.push_str("Vec<()>");
        }
        self.push_str("> + ::core::marker::Send>>");
    }

    fn print_own(&mut self, id: TypeId, submodule: bool) {
        self.src.push_str(&self.r#gen.wrpc_core_path());
        self.push_str("::ResourceOwn<");
        self.print_tyid(id, true, submodule);
        self.push_str(">");
    }

    fn print_borrow(&mut self, id: TypeId, submodule: bool) {
        self.src.push_str(&self.r#gen.wrpc_core_path());
        self.push_str("::ResourceBorrow<");
        self.print_tyid(id, true, submodule);
        self.push_str(">");
    }

    fn print_tyid(&mut self, id: TypeId, owned: bool, submodule: bool) {
        let ty = &self.resolve.types[id];
        if let Some(name) = &ty.name
            && !((self.decoding || self.encoding) && self.has_raw_strings(&Type::Id(id)))
        {
            let full_wit_type_name = full_wit_type_name(self.resolve, id);
            if let Some(TypeGeneration::Remap(remapped_path)) =
                self.r#gen.with.get(&full_wit_type_name)
            {
                let remapped_path = remapped_path.clone();
                self.push_str(&remapped_path);
            } else {
                let name = self.type_path_with_name(id, to_upper_camel_case(name), submodule);
                self.push_str(&name);
            }
            return;
        }
        match &ty.kind {
            TypeDefKind::List(ty) => self.print_list(ty, owned, submodule),
            TypeDefKind::FixedLengthList(ty, size) => {
                self.print_fixed_length_list(ty, *size, owned, submodule)
            }
            TypeDefKind::Map(key, value) => self.print_map(key, value, owned, submodule),
            TypeDefKind::Option(ty) => self.print_option(ty, owned, submodule),
            TypeDefKind::Result(ty) => self.print_result(ty, owned, submodule),
            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),
            TypeDefKind::Tuple(ty) => self.print_tuple(ty, owned, submodule),
            TypeDefKind::Resource => panic!("unsupported anonymous type reference: resource"),
            TypeDefKind::Record(_) => panic!("unsupported anonymous type reference: record"),
            TypeDefKind::Flags(_) => panic!("unsupported anonymous type reference: flags"),
            TypeDefKind::Enum(_) => panic!("unsupported anonymous type reference: enum"),
            TypeDefKind::Future(ty) => self.print_future(ty, submodule),
            TypeDefKind::Stream(ty) => self.print_stream(ty, submodule),
            TypeDefKind::Handle(Handle::Own(id)) => self.print_own(*id, submodule),
            TypeDefKind::Handle(Handle::Borrow(id)) => self.print_borrow(*id, submodule),
            TypeDefKind::Type(t) => self.print_ty(t, owned, submodule),
            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn int_repr(&mut self, repr: Int) {
        self.push_str(int_repr(repr));
    }

    fn name_of(&self, ty: TypeId) -> Option<String> {
        (self.r#gen.opts.generate_unused_types
            // If this type isn't actually used, no need to generate it.
            || matches!(
                self.info(ty),
                TypeInfo { owned: true, .. } | TypeInfo { borrowed: true, .. }
            ))
        .then(|| to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap()))
    }

    fn has_debug(&self, ty: &Type) -> bool {
        let Type::Id(id) = ty else {
            return true;
        };
        match &self.resolve.types[*id].kind {
            TypeDefKind::Future(..) | TypeDefKind::Stream(..) => false,
            TypeDefKind::List(ty)
            | TypeDefKind::FixedLengthList(ty, ..)
            | TypeDefKind::Map(_, ty)
            | TypeDefKind::Option(ty)
            | TypeDefKind::Type(ty) => self.has_debug(ty),
            TypeDefKind::Result(Result_ { ok, err }) => {
                ok.iter().chain(err).all(|ty| self.has_debug(ty))
            }
            TypeDefKind::Tuple(Tuple { types }) => types.iter().all(|ty| self.has_debug(ty)),
            _ => true,
        }
    }

    fn print_rust_enum_debug<'b>(
        &mut self,
        name: &str,
        cases: impl IntoIterator<Item = (String, Option<&'b Type>)>,
    ) where
        Self: Sized,
    {
        self.push_str("impl");
        self.push_str(" ::core::fmt::Debug for ");
        self.push_str(name);
        self.push_str(" {\n");
        self.push_str(
            "fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
        );
        self.push_str("match self {\n");
        for (case_name, payload) in cases {
            self.push_str(name);
            self.push_str("::");
            self.push_str(&case_name);
            let elide = payload.is_some_and(|ty| !self.has_debug(ty));
            if payload.is_some() {
                if elide {
                    self.push_str("(_)");
                } else {
                    self.push_str("(e)");
                }
            }
            self.push_str(" => {\n");
            self.push_str(&format!("f.debug_tuple(\"{name}::{case_name}\")"));
            if payload.is_some() {
                if elide {
                    self.push_str(r#".field(&"<async>")"#);
                } else {
                    self.push_str(".field(e)");
                }
            }
            self.push_str(".finish()\n");
            self.push_str("}\n");
        }
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn path_to_interface(&self, interface: InterfaceId) -> Option<String> {
        let InterfaceName { path, remapped } = &self.r#gen.interface_names[&interface];
        if *remapped {
            let mut path_to_root = self.path_to_root();
            path_to_root.push_str(path);
            Some(path_to_root)
        } else {
            let mut full_path = String::new();
            if let Identifier::Interface(cur, name) = self.identifier {
                if cur == interface {
                    return None;
                }
                if !self.in_import {
                    full_path.push_str("super::");
                }
                match name {
                    WorldKey::Name(_) => {
                        full_path.push_str("super::");
                    }
                    WorldKey::Interface(_) => {
                        full_path.push_str("super::super::super::");
                    }
                }
            }
            full_path.push_str(path);
            Some(full_path)
        }
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
        ty_name: &str,
        Record { fields, .. }: &Record,
        docs: &Docs,
    ) {
        let info = self.info(id);
        // We use a BTree set to make sure we don't have any duplicates and we have a stable order
        let additional_derives: BTreeSet<String> = self
            .r#gen
            .opts
            .additional_derive_attributes
            .iter()
            .cloned()
            .collect();
        if let Some(name) = self.name_of(id) {
            let (paths, _) = async_paths_tyid(self.resolve, id);

            self.rustdoc(docs);
            let mut derives = BTreeSet::new();
            if !self
                .r#gen
                .opts
                .additional_derive_ignore
                .contains(&name.to_kebab_case())
            {
                derives.extend(additional_derives.clone());
            }
            if info.is_copy() && paths.is_empty() {
                self.push_str("#[repr(C)]\n");
                if !derives.contains("Clone")
                    && !derives.contains("core :: clone :: Clone")
                    && !derives.contains(":: core :: clone :: Clone")
                {
                    derives.insert("::core::clone::Clone".to_string());
                }
                if !derives.contains("Copy")
                    && !derives.contains("core :: marker :: Copy")
                    && !derives.contains(":: core :: marker :: Copy")
                {
                    derives.insert("::core::marker::Copy".to_string());
                }
            } else if info.is_clone()
                && paths.is_empty()
                && !derives.contains("Clone")
                && !derives.contains("core :: clone :: Clone")
                && !derives.contains(":: core :: clone :: Clone")
            {
                derives.insert("::core::clone::Clone".to_string());
            }
            if !derives.is_empty() {
                self.push_str("#[derive(");
                self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
                self.push_str(")]\n");
            }
            uwriteln!(self.src, "pub struct {name} {{");
            for Field { name, ty, docs, .. } in fields {
                self.rustdoc(docs);
                self.push_str("pub ");
                self.push_str(&to_rust_ident(name));
                self.push_str(": ");
                self.print_ty(ty, true, false);
                self.push_str(",\n");
            }
            self.push_str("}\n");

            let mod_name = to_rust_ident(ty_name);

            let bytes = self.r#gen.bytes_path();
            let tokio_util = self.r#gen.tokio_util_path();
            let wrpc_core = self.r#gen.wrpc_core_path();

            let (paths, _) = async_paths_tyid(self.resolve, id);
            if paths.is_empty() {
                uwriteln!(
                    self.src,
                    r"
impl {wrpc_core}::Encode for &self::{name}
{{
    type Encoder = {mod_name}::Encoder;
}}",
                );
            }
            uwriteln!(
                self.src,
                r"
impl {wrpc_core}::Encode for self::{name}
{{
    type Encoder = {mod_name}::Encoder;
}}

impl {wrpc_core}::Decode for self::{name}
{{
    type Decoder = {mod_name}::Decoder;
    type ListDecoder = {wrpc_core}::ListDecoder<Self::Decoder>;
}}

mod {mod_name} {{
    #[derive(::core::default::Default)]
    pub struct Encoder
    {{",
            );

            for Field { name, ty, .. } in fields {
                let name = to_rust_ident(name);
                uwrite!(self.src, "{name}: <");
                self.print_encoded_ty(ty, true);
                uwriteln!(self.src, " as {wrpc_core}::Encode>::Encoder,");
            }
            uwriteln!(
                self.src,
                r"}}

    #[automatically_derived]
    impl {wrpc_core}::Transmit for Encoder
    {{
        fn poll_transmit(&mut self, cx: &mut ::core::task::Context<'_>, path: &mut ::std::vec::Vec<usize>, dst: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            let mut ready = true;"
            );
            for (i, Field { name, .. }) in fields.iter().enumerate() {
                let name = to_rust_ident(name);
                uwriteln!(
                    self.src,
                    r"
            path.push({i});
            let res = {wrpc_core}::Transmit::poll_transmit(&mut self.{name}, cx, path, dst);
            path.pop();
            let res = res?;
            ready &= res.is_ready();"
                );
            }
            let complete = fields
                .iter()
                .map(|Field { name, .. }| {
                    format!(
                        "{wrpc_core}::Transmit::is_complete(&self.{})",
                        to_rust_ident(name)
                    )
                })
                .collect::<Vec<_>>()
                .join(" && ");
            uwriteln!(
                self.src,
                r"
            if ready {{
                ::core::task::Poll::Ready(Ok(()))
            }} else {{
                ::core::task::Poll::Pending
            }}
        }}

        fn is_complete(&self) -> bool {{
            {complete}
        }}
    }}

    #[derive(::core::default::Default)]
    pub struct Decoder
    {{"
            );

            for Field { name, ty, .. } in fields {
                let name = to_rust_ident(name);
                uwrite!(self.src, "c_{name}: <");
                self.print_decoded_ty(ty, true);
                uwriteln!(self.src, " as {wrpc_core}::Decode>::Decoder,");
                uwrite!(self.src, "v_{name}: ::core::option::Option<");
                self.print_decoded_ty(ty, true);
                uwriteln!(self.src, ">,");
            }
            uwriteln!(
                self.src,
                r"}}

    #[automatically_derived]
    impl {wrpc_core}::Receive for Decoder
    {{
        fn poll_receive(&mut self, cx: &mut ::core::task::Context<'_>, path: &[usize], src: &mut {bytes}::BytesMut) -> ::core::task::Poll<::core::result::Result<(), {wrpc_core}::ReceiveError>> {{
            match path.split_first() {{"
            );
            for (i, Field { name, .. }) in fields.iter().enumerate() {
                let name = to_rust_ident(name);
                uwriteln!(
                    self.src,
                    "Some(({i}, rest)) => {wrpc_core}::Receive::poll_receive(&mut self.c_{name}, cx, rest, src),"
                );
            }
            let complete = fields
                .iter()
                .map(|Field { name, .. }| {
                    format!(
                        "{wrpc_core}::Receive::is_complete(&self.c_{})",
                        to_rust_ident(name)
                    )
                })
                .collect::<Vec<_>>()
                .join(" && ");
            uwriteln!(
                self.src,
                r#"_ => ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_core}::ReceiveError::UnknownPath)),
            }}
        }}

        fn is_complete(&self) -> bool {{
            {complete}
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Decoder for Decoder
    {{
        type Item = super::{name};
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{"#
            );
            for Field { name, .. } in fields {
                let name = to_rust_ident(name);
                uwriteln!(
                    self.src,
                    r"
                if self.v_{name}.is_none() {{
                    let Some(v) = self.c_{name}.decode(src)? else {{
                        return Ok(None)
                    }};
                    self.v_{name} = Some(v);
                }}"
                );
            }
            self.push_str("Ok(Some(Self::Item{\n");
            for Field { name, ty, .. } in fields {
                let name = to_rust_ident(name);
                let v = self.decoded_value(ty, &format!("self.v_{name}.take().unwrap()"), true);
                uwriteln!(self.src, "{name}: {v},");
            }
            self.push_str("}))\n");
            self.push_str("}\n");
            self.push_str("}\n");

            let mut names = vec![format!("super::{name}")];
            if paths.is_empty() {
                names.push(format!("&super::{name}"));
            }
            for name in names {
                let by_ref = name.starts_with('&');
                uwriteln!(
                    self.src,
                    r#"
    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<{name}> for Encoder 
    {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: {name}, dst: &mut {bytes}::BytesMut) -> ::std::io::Result<()> {{"#
                );
                self.push_str("let ");
                self.push_str(name.trim_start_matches('&'));
                self.push_str("{\n");
                for Field { name, .. } in fields {
                    let name = to_rust_ident(name);
                    uwriteln!(self.src, "{name}: f_{name},");
                }
                self.push_str("} = item;\n");

                for Field { name, ty, .. } in fields {
                    let name = to_rust_ident(name);
                    let v = self.encoded_value(ty, &format!("f_{name}"), by_ref, true);
                    uwriteln!(self.src, "self.{name}.encode({v}, dst)?;");
                }
                uwriteln!(
                    self.src,
                    r"
        Ok(())
    }}
}}",
                );
            }
            uwriteln!(self.src, "}}");

            self.push_str("impl");
            self.push_str(" ::core::fmt::Debug for ");
            self.push_str(&name);
            self.push_str(" {\n");
            self.push_str(
                "fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
            );
            self.push_str(&format!("f.debug_struct(\"{name}\")"));
            for field in fields {
                if !self.has_debug(&field.ty) {
                    self.push_str(&format!(r#".field("{}", &"<async>")"#, field.name));
                } else {
                    self.push_str(&format!(
                        ".field(\"{}\", &self.{})",
                        field.name,
                        to_rust_ident(&field.name)
                    ));
                }
            }
            self.push_str(".finish()\n");
            self.push_str("}\n");
            self.push_str("}\n");

            if info.error {
                self.push_str("impl");
                self.push_str(" ::core::fmt::Display for ");
                self.push_str(&name);
                self.push_str(" {\n");
                self.push_str(
                    "fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
                );
                self.push_str("write!(f, \"{:?}\", self)\n");
                self.push_str("}\n");
                self.push_str("}\n");
                self.push_str("impl ::core::error::Error for ");
                self.push_str(&name);
                self.push_str(" {}\n");
            }
        }
    }

    fn define_type(&mut self, name: &str, id: TypeId) {
        let equal = self.r#gen.types.get_representative_type(id);
        if !self.r#gen.opts.merge_structurally_equal_types() || equal == id {
            wit_bindgen_core::define_type(self, name, id);
        } else {
            let docs = &self.resolve.types[id].docs;
            self.type_alias(id, name, &Type::Id(equal), docs);
        }
    }

    fn type_resource(&mut self, _id: TypeId, name: &str, docs: &Docs) {
        self.rustdoc(docs);
        uwriteln!(
            self.src,
            r"
#[repr(transparent)]
pub struct {}(());",
            to_upper_camel_case(name)
        );
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_tuple(tuple, true, false);
            self.push_str(";\n");
        }
    }

    fn type_flags(&mut self, id: TypeId, ty_name: &str, flags: &Flags, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            let bitflags = self.r#gen.bitflags_path();
            let bytes = self.r#gen.bytes_path();
            let tokio_util = self.r#gen.tokio_util_path();
            let wasm_tokio = self.r#gen.wasm_tokio_path();
            let wrpc_core = self.r#gen.wrpc_core_path();

            let mod_name = to_rust_ident(ty_name);

            uwriteln!(self.src, "{bitflags}::bitflags! {{");
            self.rustdoc(docs);
            let repr = RustFlagsRepr::new(flags);
            uwriteln!(
                self.src,
                "#[derive(::core::cmp::PartialEq, ::core::cmp::Eq, ::core::cmp::PartialOrd, ::core::cmp::Ord, ::core::hash::Hash, ::core::fmt::Debug, ::core::clone::Clone, ::core::marker::Copy)]"
            );
            uwriteln!(self.src, "pub struct {name}: {repr} {{");

            for (i, flag) in flags.flags.iter().enumerate() {
                self.rustdoc(&flag.docs);
                uwriteln!(
                    self.src,
                    "const {} = 1 << {i};",
                    flag.name.to_shouty_snake_case(),
                );
            }
            uwriteln!(self.src, "}}");
            uwriteln!(self.src, "}}");

            let n = flags.flags.len();
            let n = if n.is_multiple_of(8) {
                n / 8
            } else {
                n / 8 + 1
            };
            uwriteln!(
                self.src,
                r#"
impl {wrpc_core}::Encode for self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Encode for &self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Encode for &&self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Decode for self::{name} {{
    type Decoder = {mod_name}::Codec;
    type ListDecoder = {wrpc_core}::SyncCodec<{wasm_tokio}::CoreVecDecoder<Self::Decoder>>;
}}

impl {wrpc_core}::Receive for {mod_name}::Codec {{
    fn poll_receive(&mut self, _: &mut ::core::task::Context<'_>, _: &[usize], _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::core::result::Result<(), {wrpc_core}::ReceiveError>> {{
        ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_core}::ReceiveError::UnknownPath))
    }}

    fn is_complete(&self) -> bool {{
        true
    }}
}}

use {bitflags} as __{mod_name}__bitflags;

mod {mod_name} {{
    #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug, ::core::default::Default, ::core::cmp::PartialEq, ::core::cmp::Eq)]
    pub struct Codec;

    #[automatically_derived]
    impl {wrpc_core}::Transmit for Codec {{
        fn poll_transmit(&mut self, _: &mut ::core::task::Context<'_>, _: &mut ::std::vec::Vec<usize>, _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            ::core::task::Poll::Ready(::core::result::Result::Ok(()))
        }}

        fn is_complete(&self) -> bool {{
            true
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Decoder for Codec {{
        type Item = super::{name};
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{
            let n = src.len();
            if n < {n} {{
                src.reserve({n} - n);
                return Ok(None)
            }}
            let mut bits = <<super::{name} as super::__{mod_name}__bitflags::Flags>::Bits as super::__{mod_name}__bitflags::Bits>::EMPTY.to_le_bytes();
            bits[..{n}].copy_from_slice(&src.split_to({n}));
            if let Some(item) = <super::{name} as super::__{mod_name}__bitflags::Flags>::from_bits(<super::{name} as super::__{mod_name}__bitflags::Flags>::Bits::from_le_bytes(bits)) {{
                Ok(Some(item))
            }} else {{
                Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, "invalid bits"))
            }}
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            dst.extend_from_slice(&item.bits().to_le_bytes()[..{n}]);
            Ok(())
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(*item, dst)
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &&super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(**item, dst)
        }}
    }}
}}"#,
            );
        }
    }

    fn type_variant(
        &mut self,
        id: TypeId,
        ty_name: &str,
        Variant { cases, .. }: &Variant,
        docs: &Docs,
    ) {
        let info = self.info(id);
        // We use a BTree set to make sure we don't have any duplicates and have a stable order
        let additional_derives: BTreeSet<String> = self
            .r#gen
            .opts
            .additional_derive_attributes
            .iter()
            .cloned()
            .collect();
        if let Some(name) = self.name_of(id) {
            let (paths, fut) = async_paths_tyid(self.resolve, id);

            self.rustdoc(docs);
            let mut derives = BTreeSet::new();
            if !self
                .r#gen
                .opts
                .additional_derive_ignore
                .contains(&name.to_kebab_case())
            {
                derives.extend(additional_derives.clone());
            }
            if info.is_copy() && paths.is_empty() && !fut {
                derives.extend(
                    ["::core::marker::Copy", "::core::clone::Clone"]
                        .into_iter()
                        .map(std::string::ToString::to_string),
                );
            } else if info.is_clone() && paths.is_empty() && !fut {
                derives.insert("::core::clone::Clone".to_string());
            }
            if !derives.is_empty() {
                self.push_str("#[derive(");
                self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
                self.push_str(")]\n");
            }
            uwriteln!(self.src, "pub enum {name} {{");
            for Case { name, ty, .. } in cases {
                self.rustdoc(docs);
                self.push_str(&name.to_upper_camel_case());
                if let Some(ty) = ty {
                    self.push_str("(");
                    self.print_ty(ty, true, false);
                    self.push_str(")");
                }
                self.push_str(",\n");
            }
            self.push_str("}\n");

            self.print_rust_enum_debug(
                &name,
                cases
                    .iter()
                    .map(|Case { name, ty, .. }| (name.to_upper_camel_case(), ty.as_ref())),
            );

            if info.error {
                self.push_str("impl");
                self.push_str(" ::core::fmt::Display for ");
                self.push_str(&name);
                self.push_str(" {\n");
                self.push_str(
                    "fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
                );
                self.push_str("write!(f, \"{:?}\", self)\n");
                self.push_str("}\n");
                self.push_str("}\n");
                self.push_str("\n");

                self.push_str("impl");
                self.push_str(" ::core::error::Error for ");
                self.push_str(&name);
                self.push_str(" {}\n");
            }

            let bytes = self.r#gen.bytes_path();
            let tokio_util = self.r#gen.tokio_util_path();
            let wasm_tokio = self.r#gen.wasm_tokio_path();
            let wrpc_core = self.r#gen.wrpc_core_path();

            let mod_name = to_rust_ident(ty_name);

            if cases.iter().all(|Case { ty, .. }| ty.is_none()) {
                uwriteln!(
                    self.src,
                    r#"
impl {wrpc_core}::Encode for self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Encode for &self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Encode for &&self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Decode for self::{name} {{
    type Decoder = {mod_name}::Codec;
    type ListDecoder = {wrpc_core}::SyncCodec<{wasm_tokio}::CoreVecDecoder<Self::Decoder>>;
}}

impl {wrpc_core}::Receive for {mod_name}::Codec {{
    fn poll_receive(&mut self, _: &mut ::core::task::Context<'_>, _: &[usize], _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::core::result::Result<(), {wrpc_core}::ReceiveError>> {{
        ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_core}::ReceiveError::UnknownPath))
    }}

    fn is_complete(&self) -> bool {{
        true
    }}
}}

mod {mod_name} {{
    #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug, ::core::default::Default, ::core::cmp::PartialEq, ::core::cmp::Eq)]
    pub struct Codec;

    #[automatically_derived]
    impl {wrpc_core}::Transmit for Codec {{
        fn poll_transmit(&mut self, _: &mut ::core::task::Context<'_>, _: &mut ::std::vec::Vec<usize>, _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            ::core::task::Poll::Ready(::core::result::Result::Ok(()))
        }}

        fn is_complete(&self) -> bool {{
            true
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Decoder for Codec {{
        type Item = super::{name};
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{
            let Some(disc) = {wasm_tokio}::Leb128DecoderU32.decode(src)? else {{
                return Ok(None)
            }};
            match disc {{"#
                );
                for (i, case) in cases.iter().enumerate() {
                    let case = case.name.to_upper_camel_case();
                    uwriteln!(self.src, "{i} => Ok(Some(super::{name}::{case})),");
                }
                uwriteln!(
                    self.src,
                    r#"_ => Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, format!("unknown variant discriminant `{{disc}}`"))),
            }}
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            {wasm_tokio}::Leb128Encoder.encode(match item {{"#
                );
                for (i, case) in cases.iter().enumerate() {
                    let case = case.name.to_upper_camel_case();
                    uwriteln!(self.src, "super::{name}::{case} => {i}_u32,");
                }
                uwriteln!(
                    self.src,
                    r#"
            }}, dst)
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(*item, dst)
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &&super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(**item, dst)
        }}
    }}
}}"#,
                );
            } else {
                let (paths, fut) = async_paths_tyid(self.resolve, id);
                if paths.is_empty() && !fut {
                    uwrite!(
                        self.src,
                        r#"
impl {wrpc_core}::Encode for &self::{name}
{{
    type Encoder = {mod_name}::Encoder;
}}

impl {wrpc_core}::Encode for &&self::{name}
{{
    type Encoder = {mod_name}::Encoder;
}}
"#
                    );
                }

                uwrite!(
                    self.src,
                    r#"
impl {wrpc_core}::Encode for self::{name}
{{
    type Encoder = {mod_name}::Encoder;
}}

impl {wrpc_core}::Decode for self::{name}
{{
    type Decoder = {mod_name}::Decoder;
    type ListDecoder = {wrpc_core}::ListDecoder<Self::Decoder>;
}}

mod {mod_name} {{
    #[derive(::core::default::Default)]
    pub struct Encoder(::core::option::Option<PayloadEncoder>);

    pub enum PayloadEncoder
    {{"#
                );
                let mut poll_transmit = String::default();
                let mut is_complete = String::default();
                for Case {
                    name: case_name,
                    ty,
                    ..
                } in cases
                {
                    if let Some(ty) = ty {
                        let case = case_name.to_upper_camel_case();
                        uwrite!(self.src, "{case}(<");
                        self.print_encoded_ty(ty, true);
                        uwriteln!(self.src, " as {wrpc_core}::Encode>::Encoder),");
                        poll_transmit.push_str(&format!(
                            "Some(PayloadEncoder::{case}(enc)) => {wrpc_core}::Transmit::poll_transmit(enc, cx, path, dst),"
                        ));
                        is_complete.push_str(&format!(
                            "Some(PayloadEncoder::{case}(enc)) => {wrpc_core}::Transmit::is_complete(enc),"
                        ));
                    }
                }
                uwrite!(
                    self.src,
                    r#"}}

    #[automatically_derived]
    impl {wrpc_core}::Transmit for Encoder
    {{
        fn poll_transmit(&mut self, cx: &mut ::core::task::Context<'_>, path: &mut ::std::vec::Vec<usize>, dst: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            match &mut self.0 {{
                {poll_transmit}
                None => ::core::task::Poll::Ready(Ok(())),
            }}
        }}

        fn is_complete(&self) -> bool {{
            match &self.0 {{
                {is_complete}
                None => true,
            }}
        }}
    }}"#
                );

                let mut names = vec![format!("super::{name}")];
                if paths.is_empty() && !fut {
                    names.extend_from_slice(&[
                        format!("&super::{name}"),
                        format!("&&super::{name}"),
                    ]);
                }
                for ty in names {
                    let by_ref = ty.starts_with('&');
                    uwrite!(
                        self.src,
                        r#"
    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<{ty}> for Encoder 
    {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: {ty}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            match item {{"#
                    );
                    for (
                        i,
                        Case {
                            name: case_name,
                            ty,
                            ..
                        },
                    ) in cases.iter().enumerate()
                    {
                        let case = case_name.to_upper_camel_case();
                        if let Some(ty) = ty {
                            let payload = self.encoded_value(ty, "payload", by_ref, true);
                            uwrite!(
                                self.src,
                                r"
                super::{name}::{case}(payload) => {{
                    {wasm_tokio}::Leb128Encoder.encode({i}_u32, dst)?;
                    let mut enc = ::core::default::Default::default();
                    {tokio_util}::codec::Encoder::encode(&mut enc, {payload}, dst)?;
                    self.0 = Some(PayloadEncoder::{case}(enc));
                    Ok(())
                }},"
                            );
                        } else {
                            uwrite!(
                                self.src,
                                "
                super::{name}::{case} => {{
                    self.0 = None;
                    {wasm_tokio}::Leb128Encoder.encode({i}_u32, dst)
                }},"
                            );
                        }
                    }
                    uwrite!(
                        self.src,
                        r"
            }}
        }}
    }}"
                    );
                }

                uwrite!(
                    self.src,
                    r"
    pub enum PayloadDecoder
    {{"
                );

                for Case {
                    name: case_name,
                    ty,
                    ..
                } in cases
                {
                    if let Some(ty) = ty {
                        let case = case_name.to_upper_camel_case();
                        uwrite!(
                            self.src,
                            r"
                {case}(<"
                        );
                        self.print_decoded_ty(ty, true);
                        uwriteln!(self.src, " as {wrpc_core}::Decode>::Decoder),");
                    }
                }

                uwrite!(
                    self.src,
                    r#"
    }}

    pub enum Decoder
    {{
        Decoding(PayloadDecoder),
        Decoded(::core::option::Option<PayloadDecoder>),
    }}

    #[automatically_derived]
    impl ::core::default::Default for Decoder
    {{
        fn default() -> Self {{
            Self::Decoded(None)
        }}
    }}
"#
                );
                let mut poll_receive = String::default();
                let mut is_complete = String::default();
                for Case {
                    name: case_name,
                    ty,
                    ..
                } in cases
                {
                    if ty.is_some() {
                        let case = case_name.to_upper_camel_case();
                        poll_receive.push_str(&format!(
                            "Self::Decoded(Some(PayloadDecoder::{case}(dec))) => {wrpc_core}::Receive::poll_receive(dec, cx, path, src),"
                        ));
                        is_complete.push_str(&format!(
                            "Self::Decoded(Some(PayloadDecoder::{case}(dec))) => {wrpc_core}::Receive::is_complete(dec),"
                        ));
                    }
                }
                uwriteln!(
                    self.src,
                    r#"
    #[automatically_derived]
    impl {wrpc_core}::Receive for Decoder
    {{
        fn poll_receive(&mut self, cx: &mut ::core::task::Context<'_>, path: &[usize], src: &mut {bytes}::BytesMut) -> ::core::task::Poll<::core::result::Result<(), {wrpc_core}::ReceiveError>> {{
            match self {{
                {poll_receive}
                _ => ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_core}::ReceiveError::UnknownPath)),
            }}
        }}

        fn is_complete(&self) -> bool {{
            match self {{
                {is_complete}
                _ => true,
            }}
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Decoder for Decoder
    {{
        type Item = super::{name};
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{
            if let Self::Decoded(..) = self {{
                let Some(disc) = {wasm_tokio}::Leb128DecoderU32.decode(src)? else {{
                    return Ok(None)
                }};
                let dec = match disc {{"#
                );
                for (
                    i,
                    Case {
                        name: case_name,
                        ty,
                        ..
                    },
                ) in cases.iter().enumerate()
                {
                    let case = case_name.to_upper_camel_case();
                    if ty.is_some() {
                        uwrite!(
                            self.src,
                            r"
                    {i} => PayloadDecoder::{case}(::core::default::Default::default()),"
                        );
                    } else {
                        uwrite!(
                            self.src,
                            "
                    {i} => {{
                        *self = Self::Decoded(None);
                        return Ok(Some(super::{name}::{case}))
                    }},"
                        );
                    }
                }
                uwrite!(
                    self.src,
                    r#"
                    _ => return Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, format!("unknown variant discriminant `{{disc}}`"))),
                }};
                *self = Self::Decoding(dec);
            }}
            let Self::Decoding(state) = self else {{
                unreachable!()
            }};
            let item = match state {{"#
                );
                for Case {
                    name: case_name,
                    ty,
                    ..
                } in cases
                {
                    let case = case_name.to_upper_camel_case();
                    if let Some(ty) = ty {
                        let payload = self.decoded_value(ty, "payload", true);
                        uwriteln!(
                            self.src,
                            r"
                        PayloadDecoder::{case}(dec) => {{
                            let Some(payload) = dec.decode(src)? else {{
                                return Ok(None)
                            }};
                            super::{name}::{case}({payload})
                        }},"
                        );
                    }
                }
                uwriteln!(
                    self.src,
                    r#"
            }};
            if let Self::Decoding(dec) = ::core::mem::take(self) {{
                *self = Self::Decoded(Some(dec));
            }}
            Ok(Some(item))
        }}
    }}
}}"#,
                );
            }
        }
    }

    fn type_option(&mut self, id: TypeId, _name: &str, payload: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_option(payload, true, false);
            self.push_str(";\n");
        }
    }

    fn type_result(&mut self, id: TypeId, _name: &str, result: &Result_, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_result(result, true, false);
            self.push_str(";\n");
        }
    }

    fn type_enum(&mut self, id: TypeId, ty_name: &str, enum_: &Enum, docs: &Docs) {
        let info = self.info(id);

        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            self.push_str("#[repr(");
            self.int_repr(enum_.tag());
            self.push_str(")]\n");
            // We use a BTree set to make sure we don't have any duplicates and a stable order
            let mut derives: BTreeSet<String> = BTreeSet::new();
            if !self
                .r#gen
                .opts
                .additional_derive_ignore
                .contains(&name.to_kebab_case())
            {
                derives.extend(self.r#gen.opts.additional_derive_attributes.iter().cloned());
            }
            derives.extend(
                [
                    ":: core :: clone :: Clone",
                    ":: core :: cmp :: Eq",
                    ":: core :: cmp :: Ord",
                    ":: core :: cmp :: PartialEq",
                    ":: core :: cmp :: PartialOrd",
                    ":: core :: marker :: Copy",
                ]
                .into_iter()
                .map(std::string::ToString::to_string),
            );
            self.push_str("#[derive(");
            self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
            self.push_str(")]\n");
            uwriteln!(self.src, "pub enum {name} {{");
            for case in &enum_.cases {
                self.rustdoc(&case.docs);
                self.push_str(&case.name.to_upper_camel_case());
                self.push_str(",\n");
            }
            self.push_str("}\n");

            // Auto-synthesize an implementation of the standard `Error` trait for
            // error-looking types based on their name.
            if info.error {
                self.push_str("impl ");
                self.push_str(&name);
                self.push_str("{\n");

                self.push_str("pub fn name(&self) -> &'static str {\n");
                self.push_str("match self {\n");
                for case in &enum_.cases {
                    self.push_str(&name);
                    self.push_str("::");
                    self.push_str(&case.name.to_upper_camel_case());
                    self.push_str(" => \"");
                    self.push_str(case.name.as_str());
                    self.push_str("\",\n");
                }
                self.push_str("}\n");
                self.push_str("}\n");

                self.push_str("pub fn message(&self) -> &'static str {\n");
                self.push_str("match self {\n");
                for case in &enum_.cases {
                    self.push_str(&name);
                    self.push_str("::");
                    self.push_str(&case.name.to_upper_camel_case());
                    self.push_str(" => \"");
                    if let Some(contents) = &case.docs.contents {
                        self.push_str(contents.trim());
                    }
                    self.push_str("\",\n");
                }
                self.push_str("}\n");
                self.push_str("}\n");

                self.push_str("}\n");

                self.push_str("impl ::core::fmt::Debug for ");
                self.push_str(&name);
                self.push_str(
                "{\nfn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
            );
                self.push_str("f.debug_struct(\"");
                self.push_str(&name);
                self.push_str("\")\n");
                self.push_str(".field(\"code\", &(*self as i32))\n");
                self.push_str(".field(\"name\", &self.name())\n");
                self.push_str(".field(\"message\", &self.message())\n");
                self.push_str(".finish()\n");
                self.push_str("}\n");
                self.push_str("}\n");

                self.push_str("impl ::core::fmt::Display for ");
                self.push_str(&name);
                self.push_str(
                "{\nfn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
            );
                self.push_str("write!(f, \"{} (error {})\", self.name(), *self as i32)\n");
                self.push_str("}\n");
                self.push_str("}\n");
                self.push_str("\n");
                self.push_str("impl ::core::error::Error for ");
                self.push_str(&name);
                self.push_str(" {}\n");
            } else {
                self.print_rust_enum_debug(
                    &name,
                    enum_
                        .cases
                        .iter()
                        .map(|c| (c.name.to_upper_camel_case(), None)),
                );
            }

            let bytes = self.r#gen.bytes_path();
            let tokio_util = self.r#gen.tokio_util_path();
            let wasm_tokio = self.r#gen.wasm_tokio_path();
            let wrpc_core = self.r#gen.wrpc_core_path();

            let mod_name = to_rust_ident(ty_name);

            uwriteln!(
                self.src,
                r#"
impl {wrpc_core}::Encode for self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Encode for &self::{name} {{
    type Encoder = {mod_name}::Codec;
}}

impl {wrpc_core}::Decode for self::{name} {{
    type Decoder = {mod_name}::Codec;
    type ListDecoder = {wrpc_core}::SyncCodec<{wasm_tokio}::CoreVecDecoder<Self::Decoder>>;
}}

impl {wrpc_core}::Receive for {mod_name}::Codec {{
    fn poll_receive(&mut self, _: &mut ::core::task::Context<'_>, _: &[usize], _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::core::result::Result<(), {wrpc_core}::ReceiveError>> {{
        ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_core}::ReceiveError::UnknownPath))
    }}

    fn is_complete(&self) -> bool {{
        true
    }}
}}

mod {mod_name} {{
    #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug, ::core::default::Default, ::core::cmp::PartialEq, ::core::cmp::Eq)]
    pub struct Codec;

    #[automatically_derived]
    impl {wrpc_core}::Transmit for Codec {{
        fn poll_transmit(&mut self, _: &mut ::core::task::Context<'_>, _: &mut ::std::vec::Vec<usize>, _: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            ::core::task::Poll::Ready(::core::result::Result::Ok(()))
        }}

        fn is_complete(&self) -> bool {{
            true
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Decoder for Codec {{
        type Item = super::{name};
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{
            let Some(disc) = {wasm_tokio}::Leb128DecoderU32.decode(src)? else {{
                return Ok(None)
            }};
            match disc {{"#
            );
            for (i, case) in enum_.cases.iter().enumerate() {
                let case = case.name.to_upper_camel_case();
                uwriteln!(self.src, "{i} => Ok(Some(super::{name}::{case})),");
            }
            uwriteln!(
                self.src,
                r#"_ => Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, format!("unknown enum discriminant `{{disc}}`"))),
            }}
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            {wasm_tokio}::Leb128Encoder.encode(match item {{"#
            );
            for (i, case) in enum_.cases.iter().enumerate() {
                let case = case.name.to_upper_camel_case();
                uwriteln!(self.src, "super::{name}::{case} => {i}_u32,");
            }
            uwriteln!(
                self.src,
                r#"
            }}, dst)
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(*item, dst)
        }}
    }}

    #[automatically_derived]
    impl {tokio_util}::codec::Encoder<&&super::{name}> for Codec {{
        type Error = ::std::io::Error;

        fn encode(&mut self, item: &&super::{name}, dst: &mut {bytes}::BytesMut) -> ::core::result::Result<(), Self::Error> {{
            self.encode(**item, dst)
        }}
    }}
}}"#,
            );
        }
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_ty(ty, true, false);
            self.push_str(";\n");
        }
    }

    fn type_list(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_list(ty, true, false);
            self.push_str(";\n");
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
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_fixed_length_list(ty, size, true, false);
            self.push_str(";\n");
        }
    }

    fn type_map(&mut self, id: TypeId, _name: &str, key: &Type, value: &Type, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_map(key, value, true, false);
            self.push_str(";\n");
        }
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.rustdoc(docs);
        uwrite!(self.src, "pub type {} = ", name.to_upper_camel_case());
        self.print_ty(ty, true, false);
        self.src.push_str(";\n");
    }

    fn type_future(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_future(ty, false);
            self.push_str(";\n");
        }
    }

    fn type_stream(&mut self, id: TypeId, _name: &str, ty: &Option<Type>, docs: &Docs) {
        if let Some(name) = self.name_of(id) {
            self.rustdoc(docs);
            uwrite!(self.src, "pub type {name} = ");
            self.print_stream(ty, false);
            self.push_str(";\n");
        }
    }
}
