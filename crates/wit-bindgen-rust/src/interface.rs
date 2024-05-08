use crate::{
    int_repr, to_rust_ident, to_upper_camel_case, FnSig, Identifier, InterfaceName, Ownership,
    RustFlagsRepr, RustWrpc,
};
use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::mem;
use wit_bindgen_core::{
    dealias, uwrite, uwriteln,
    wit_parser::{
        Docs, Enum, EnumCase, Field, Flags, Function, FunctionKind, Handle, Int, InterfaceId,
        Record, Resolve, Result_, Results, Tuple, Type, TypeDefKind, TypeId, TypeOwner, Variant,
        World, WorldKey,
    },
    Source, TypeInfo,
};

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) gen: &'a mut RustWrpc,
    pub resolve: &'a Resolve,
}

/// A description of the "mode" in which a type is printed.
///
/// Rust types can either be "borrowed" or "owned". This primarily has to do
/// with lists and imports where arguments to imports can be borrowed lists in
/// theory as ownership is not taken from the caller. This structure is used to
/// help with this fact in addition to the various codegen options of this
/// generator. Namely types in WIT can be reflected into Rust as two separate
/// types, one "owned" and one "borrowed" (aka one with `Vec` and one with
/// `&[T]`).
///
/// This structure is used in conjunction with `modes_of` and `type_mode_for*`
/// primarily. That enables creating a programmatic description of what a type
/// is rendered as along with various options.
///
/// Note that a `TypeMode` is a description of a single "level" of a type. This
/// means that there's one mode for `Vec<T>` and one mode for `T` internally.
/// This is mostly used for things like records where some fields have lifetime
/// parameters, for example, and others don't.
///
/// This type is intended to simplify generation of types and encapsulate all
/// the knowledge about whether lifetime parameters are used and how lists are
/// rendered.
///
/// There are currently two users of lifetime parameters:
///
/// * Lists - when borrowed these are rendered as either `&[T]` or `&str`.
/// * Borrowed resources - for resources owned by the current module they're
///   represented as `&T` and for borrows of imported resources they're
///   represented, more-or-less, as `&Resource<T>`.
///
/// Lists have a choice of being rendered as borrowed or not but resources are
/// required to be borrowed.
#[derive(Debug, Copy, Clone, PartialEq)]
struct TypeMode {
    /// The lifetime parameter, if any, for this type. If present this type is
    /// required to have a lifetime parameter.
    lifetime: Option<&'static str>,

    /// Whether or not lists are borrowed in this type.
    ///
    /// If this field is `true` then lists are rendered as `&[T]` and `&str`
    /// rather than their owned equivalent. If this field is `false` than the
    /// owned equivalents are used instead.
    lists_borrowed: bool,

    /// The "style" of ownership that this mode was created with.
    ///
    /// This information is used to determine what mode the next layer deep int
    /// he type tree is rendered with. For example if this layer is owned so is
    /// the next layer. This is primarily used for the "`OnlyTopBorrowed`"
    /// ownership style where all further layers beneath that are `Owned`.
    style: TypeOwnershipStyle,
}

/// The style of ownership of a type, used to initially create a `TypeMode` and
/// stored internally within it as well.
#[derive(Debug, Copy, Clone, PartialEq)]
enum TypeOwnershipStyle {
    /// This style means owned things are printed such as `Vec<T>` and `String`.
    ///
    /// Note that this primarily applies to lists.
    Owned,

    /// This style means that lists/strings are `&[T]` and `&str`.
    ///
    /// Note that this primarily applies to lists.
    Borrowed,

    /// This style means that the top-level of a type is borrowed but all other
    /// layers are `Owned`.
    ///
    /// This is used for parameters in the "owning" mode of generation to
    /// imports. It's easy enough to create a `&T` at the root layer but it's
    /// more difficult to create `&T` stored within a `U`, for example.
    OnlyTopBorrowed,
}

impl TypeMode {
    /// Returns a mode where everything is indicated that it's supposed to be
    /// rendered as an "owned" type.
    fn owned() -> TypeMode {
        TypeMode {
            lifetime: None,
            lists_borrowed: false,
            style: TypeOwnershipStyle::Owned,
        }
    }
}

impl TypeOwnershipStyle {
    /// Preserves this mode except for `OnlyTopBorrowed` where it switches it to
    /// `Owned`.
    fn next(&self) -> TypeOwnershipStyle {
        match self {
            TypeOwnershipStyle::Owned => TypeOwnershipStyle::Owned,
            TypeOwnershipStyle::Borrowed => TypeOwnershipStyle::Borrowed,
            TypeOwnershipStyle::OnlyTopBorrowed => TypeOwnershipStyle::Owned,
        }
    }
}

impl InterfaceGenerator<'_> {
    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Iterator<Item = &'a Function> + Clone,
    ) -> bool {
        let mut traits = BTreeMap::new();
        let mut funcs_to_export = Vec::new();
        let mut resources_to_drop = Vec::new();

        traits.insert(None, ("Handler".to_string(), Vec::new()));

        if let Identifier::Interface(id, ..) = identifier {
            for (name, id) in &self.resolve.interfaces[id].types {
                match self.resolve.types[*id].kind {
                    TypeDefKind::Resource => {}
                    _ => continue,
                }
                resources_to_drop.push(name);
                let camel = name.to_upper_camel_case();
                traits.insert(Some(*id), (format!("Handler{camel}"), Vec::new()));
            }
        }

        for func in funcs {
            if self.gen.skip.contains(&func.name) {
                continue;
            }

            let resource = match func.kind {
                FunctionKind::Freestanding => None,
                FunctionKind::Method(id)
                | FunctionKind::Constructor(id)
                | FunctionKind::Static(id) => Some(id),
            };
            funcs_to_export.push(func);
            let (_, methods) = traits.get_mut(&resource).unwrap();

            let prev = mem::take(&mut self.src);
            let mut sig = FnSig {
                use_item_name: true,
                private: true,
                definition: true,
                ..Default::default()
            };
            if let FunctionKind::Method(_) | FunctionKind::Freestanding = &func.kind {
                sig.self_arg = Some("&self".into());
                sig.self_is_first_param = true;
            }
            self.print_docs_and_params(func, true, &sig);
            if let FunctionKind::Constructor(_) = &func.kind {
                uwrite!(
                self.src,
                " -> impl ::core::future::Future<Output = {}::Result<Self>> + Send where Self: Sized",
                self.gen.anyhow_path()
            );
            } else {
                match func.results.len() {
                    0 => {
                        uwrite!(
                            self.src,
                            " -> impl ::core::future::Future<Output = {}::Result<()>> + Send",
                            self.gen.anyhow_path()
                        );
                    }
                    1 => {
                        uwrite!(
                            self.src,
                            " -> impl ::core::future::Future<Output = {}::Result<",
                            self.gen.anyhow_path()
                        );
                        let ty = func.results.iter_types().next().unwrap();
                        let mode = self.type_mode_for(ty, TypeOwnershipStyle::Owned, "'INVALID");
                        assert!(mode.lifetime.is_none());
                        self.print_ty(ty, mode);
                        self.push_str(">> + Send");
                    }
                    _ => {
                        uwrite!(
                            self.src,
                            " -> impl ::core::future::Future<Output = {}::Result<(",
                            self.gen.anyhow_path()
                        );
                        for ty in func.results.iter_types() {
                            let mode =
                                self.type_mode_for(ty, TypeOwnershipStyle::Owned, "'INVALID");
                            assert!(mode.lifetime.is_none());
                            self.print_ty(ty, mode);
                            self.push_str(", ");
                        }
                        self.push_str(")>> + Send");
                    }
                }
            }
            self.src.push_str(";\n");
            let trait_method = mem::replace(&mut self.src, prev);
            methods.push(trait_method);
        }

        let (name, methods) = traits.remove(&None).unwrap();
        if !methods.is_empty() || !traits.is_empty() {
            self.generate_interface_trait(
                &name,
                &methods,
                traits.iter().map(|(resource, (trait_name, _methods))| {
                    (resource.unwrap(), trait_name.as_str())
                }),
            );
        }

        for (trait_name, methods) in traits.values() {
            uwriteln!(self.src, "pub trait {trait_name}<Ctx>: 'static {{");
            for method in methods {
                self.src.push_str(method);
            }
            uwriteln!(self.src, "}}");
        }

        if !funcs_to_export
            .iter()
            .any(|Function { kind, .. }| matches!(kind, FunctionKind::Freestanding))
        {
            return false;
        }

        uwriteln!(
            self.src,
            "pub trait Server<Ctx, Tx: {wrpc_transport}::Transmitter> {{\n",
            wrpc_transport = self.gen.wrpc_transport_path()
        );
        for Function {
            kind, params, name, ..
        } in &funcs_to_export
        {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            uwrite!(
                self.src,
                r#"fn serve_{}(&self, invocation: {wrpc_transport}::AcceptedInvocation<Ctx, ("#,
                name.to_snake_case(),
                wrpc_transport = self.gen.wrpc_transport_path()
            );
            for (_, ty) in params {
                self.print_ty(ty, TypeMode::owned());
                self.push_str(",");
            }
            uwriteln!(self.src, r#"), Tx>);"#,);
        }
        uwrite!(
            self.src,
            r#"
}}

#[automatically_derived]
impl <Ctx, T, Tx> Server<Ctx, Tx> for T
    where
        Ctx: Send + 'static,
        T: Handler<Ctx> + Send + Sync + Clone + 'static,
        Tx: {wrpc_transport}::Transmitter + Send + 'static,
    {{"#,
            wrpc_transport = self.gen.wrpc_transport_path()
        );
        for Function {
            kind, params, name, ..
        } in &funcs_to_export
        {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            let method_name = format!("serve_{name}", name = name.to_snake_case());
            uwrite!(
                self.src,
                r#"
fn {method_name}(
    &self,
    {wrpc_transport}::AcceptedInvocation{{
        context,
        params,
        result_subject,
        error_subject,
        transmitter,
    }}: {wrpc_transport}::AcceptedInvocation<
        Ctx,
        ("#,
                wrpc_transport = self.gen.wrpc_transport_path()
            );
            for (_, ty) in params {
                self.print_ty(ty, TypeMode::owned());
                self.push_str(",");
            }
            uwrite!(
                self.src,
                r#"),
        Tx>) {{
            use {tracing}::Instrument as _;
            let span = {tracing}::trace_span!("{method_name}");
            let _enter = span.enter();
            let handler = self.clone();
            {tokio}::spawn(async move {{
                match handler.{name}(context, "#,
                name = to_rust_ident(name),
                tokio = self.gen.tokio_path(),
                tracing = self.gen.tracing_path(),
            );
            for i in 0..params.len() {
                self.src.push_str("params.");
                self.src.push_str(&i.to_string());
                self.src.push_str(",");
            }
            uwrite!(
                self.src,
                r#").await {{
                    Ok(res) => {{
                        if let Err(err) = transmitter.transmit_static(result_subject, res).await {{
                            {tracing}::error!(?err, "failed to transmit result")
                        }}
                    }}
                    Err(err) => {{
                        if let Err(err) = transmitter.transmit_static(error_subject, err).await {{
                            {tracing}::error!(?err, "failed to transmit error")
                        }}
                    }}
                }}
            }}.instrument({tracing}::trace_span!("{method_name}")));"#,
                tracing = self.gen.tracing_path(),
            );
            self.push_str("}\n");
        }
        self.push_str("}\n");
        uwrite!(
            self.src,
            r#"
pub async fn serve_interface<T: {wrpc_transport}::Client, U>(
    wrpc: &T,
    server: impl Server<T::Context, <T::Acceptor as {wrpc_transport}::Acceptor>::Transmitter> + Clone,
    shutdown: impl ::core::future::Future<Output = U>,
) -> {anyhow}::Result<U> {{
    use {anyhow}::Context as _;
    use {futures}::StreamExt as _;
    use {tracing}::Instrument as _;
    let span = {tracing}::trace_span!("serve_interface");
    let _enter = span.enter();
    let ("#,
            anyhow = self.gen.anyhow_path(),
            futures = self.gen.futures_path(),
            tracing = self.gen.tracing_path(),
            wrpc_transport = self.gen.wrpc_transport_path()
        );
        for Function { kind, name, .. } in &funcs_to_export {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            uwrite!(self.src, "{},", to_rust_ident(name));
        }
        uwrite!(
            self.src,
            ") = {tokio}::try_join!(",
            tokio = self.gen.tokio_path()
        );
        let instance = match identifier {
            Identifier::Interface(id, name) => {
                let interface = &self.resolve.interfaces[id];
                let name = match name {
                    WorldKey::Name(s) => s.to_string(),
                    WorldKey::Interface(..) => interface
                        .name
                        .as_ref()
                        .expect("interface name missing")
                        .to_string(),
                };
                if let Some(package) = interface.package {
                    self.resolve.id_of_name(package, &name)
                } else {
                    name
                }
            }
            Identifier::World(world) => {
                let World {
                    ref name, package, ..
                } = self.resolve.worlds[world];
                if let Some(package) = package {
                    self.resolve.id_of_name(package, name)
                } else {
                    name.to_string()
                }
            }
        };
        for Function { kind, name, .. } in &funcs_to_export {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            uwrite!(
                self.src,
                r#"async {{ wrpc.serve_static("{instance}", "{name}").await.context("failed to serve `{instance}.{name}`") }}.instrument({tracing}::trace_span!("serve_interface")),"#,
                tracing = self.gen.tracing_path(),
            );
        }
        self.push_str(")?;\n");
        for Function { kind, name, .. } in &funcs_to_export {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            let name = to_rust_ident(name);
            uwrite!(self.src, "let mut {name} = ::core::pin::pin!({name});\n",);
        }
        self.push_str("let mut shutdown = ::core::pin::pin!(shutdown);\n");
        self.push_str("'outer: loop {\n");
        uwriteln!(
            self.src,
            "{tokio}::select! {{",
            tokio = self.gen.tokio_path()
        );

        for Function { kind, name, .. } in &funcs_to_export {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            uwriteln!(
                self.src,
                "invocation = {}.next() => {{",
                to_rust_ident(name)
            );
            self.push_str("match invocation {\n");
            self.push_str("Some(Ok(invocation)) => {\n");
            uwriteln!(
                self.src,
                "server.serve_{}(invocation)",
                name.to_snake_case()
            );
            self.push_str("},\n");
            self.push_str("Some(Err(err)) => {\n");
            uwriteln!(
                self.src,
                r#"{tracing}::error!("failed to accept `{instance}.{name}` invocation");"#,
                tracing = self.gen.tracing_path()
            );
            self.push_str("},\n");
            self.push_str("None => {\n");
            uwriteln!(
                self.src,
                r#"{tracing}::warn!("`{instance}.{name}` stream unexpectedly finished, resubscribe");"#,
                tracing = self.gen.tracing_path()
            );
            self.push_str("continue 'outer\n");
            self.push_str("},\n");
            self.push_str("}\n");
            self.push_str("},\n");
        }

        self.push_str("v = &mut shutdown => {\n");
        uwriteln!(
            self.src,
            r#"{tracing}::debug!("shutdown received");"#,
            tracing = self.gen.tracing_path()
        );
        self.push_str("return Ok(v)\n");
        self.push_str("},\n");
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
        true
    }

    fn generate_interface_trait<'a>(
        &mut self,
        trait_name: &str,
        methods: &[Source],
        resource_traits: impl Iterator<Item = (TypeId, &'a str)>,
    ) {
        uwriteln!(self.src, "pub trait {trait_name}<Ctx> {{");
        for (id, trait_name) in resource_traits {
            let name = self.resolve.types[id]
                .name
                .as_ref()
                .unwrap()
                .to_upper_camel_case();
            uwriteln!(self.src, "type {name}: {trait_name}<Ctx>;");
        }
        for method in methods {
            self.src.push_str(method);
        }
        uwriteln!(self.src, "}}");
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
        let mut path_to_root = String::new();

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

    pub fn finish_append_submodule(mut self, snake: &str, module_path: Vec<String>) {
        let module = self.finish();
        let module = format!(
            "\
                #[allow(dead_code, clippy::all)]
                pub mod {snake} {{
                    {module}
                }}
",
        );
        let map = if self.in_import {
            &mut self.gen.import_modules
        } else {
            &mut self.gen.export_modules
        };
        map.push((module, module_path));
    }

    fn generate_guest_import(&mut self, instance: &str, func: &Function) {
        if self.gen.skip.contains(&func.name) {
            return;
        }

        let mut sig = FnSig::default();
        match func.kind {
            FunctionKind::Freestanding => {}
            FunctionKind::Method(id) | FunctionKind::Static(id) | FunctionKind::Constructor(id) => {
                let name = self.resolve.types[id].name.as_ref().unwrap();
                let name = to_upper_camel_case(name);
                uwriteln!(self.src, "impl {name} {{");
                sig.use_item_name = true;
                if let FunctionKind::Method(_) = &func.kind {
                    sig.self_arg = Some("&self".into());
                    sig.self_is_first_param = true;
                }
            }
        }
        self.src.push_str("#[allow(clippy::all)]\n");
        let params = self.print_signature(func, false, &sig);
        self.src.push_str("{\n");
        let anyhow = self.gen.anyhow_path();
        uwriteln!(self.src, "use {anyhow}::Context as _;");
        self.src.push_str("let wrpc__ = wrpc__.invoke_static(");
        match func.kind {
            FunctionKind::Freestanding
            | FunctionKind::Static(..)
            | FunctionKind::Constructor(..) => {
                uwrite!(self.src, r#""{instance}""#);
            }
            FunctionKind::Method(..) => {
                self.src.push_str("&self.0");
            }
        }
        self.src.push_str(r#", ""#);
        self.src.push_str(&func.name);
        self.src.push_str(r#"", "#);
        match &params[..] {
            [] => self.src.push_str("()"),
            [p] => self.src.push_str(p),
            _ => {
                self.src.push_str("(");
                for p in params {
                    uwrite!(self.src, "{p}, ");
                }
                self.src.push_str(")");
            }
        }
        // TODO: There should be a way to "split" the future and pass I/O to the caller
        uwriteln!(
            self.src,
            r#"
            ).await.context("failed to invoke `{}`")?;
        wrpc__.1.await.context("failed to transmit parameters")?;
        Ok(wrpc__.0)
    }}"#,
            func.name
        );

        match func.kind {
            FunctionKind::Freestanding => {}
            FunctionKind::Method(_) | FunctionKind::Static(_) | FunctionKind::Constructor(_) => {
                self.src.push_str("}\n");
            }
        }
    }

    pub fn generate_stub<'a>(
        &mut self,
        interface: Option<(InterfaceId, &WorldKey)>,
        funcs: impl Iterator<Item = &'a Function> + Clone,
    ) {
        let mut funcs = super::group_by_resource(funcs.clone());

        let root_methods = funcs.remove(&None).unwrap_or(Vec::new());

        let mut extra_trait_items = String::new();
        let guest_trait = match interface {
            Some((id, _)) => {
                let path = self.path_to_interface(id).unwrap();
                for (name, id) in &self.resolve.interfaces[id].types {
                    match self.resolve.types[*id].kind {
                        TypeDefKind::Resource => {}
                        _ => continue,
                    }
                    let camel = name.to_upper_camel_case();
                    uwriteln!(extra_trait_items, "type {camel} = Stub;");

                    let resource_methods = funcs.remove(&Some(*id)).unwrap_or(Vec::new());
                    let trait_name = format!("{path}::Handler{camel}");
                    self.generate_stub_impl(&trait_name, "", &resource_methods);
                }
                format!("{path}::Handler")
            }
            None => {
                assert!(funcs.is_empty());
                "Handler".to_string()
            }
        };

        if !root_methods.is_empty() || !extra_trait_items.is_empty() {
            self.generate_stub_impl(&guest_trait, &extra_trait_items, &root_methods);
        }
    }

    fn generate_stub_impl(
        &mut self,
        trait_name: &str,
        extra_trait_items: &str,
        funcs: &[&Function],
    ) {
        uwriteln!(self.src, "impl<Ctx: Send> {trait_name}<Ctx> for Stub {{");
        self.src.push_str(extra_trait_items);

        for func in funcs {
            if self.gen.skip.contains(&func.name) {
                continue;
            }
            let mut sig = FnSig {
                use_item_name: true,
                private: true,
                ..Default::default()
            };
            if let FunctionKind::Method(_) | FunctionKind::Freestanding = &func.kind {
                sig.self_arg = Some("&self".into());
                sig.self_is_first_param = true;
            }
            self.print_signature(func, true, &sig);
            self.src.push_str("{ unreachable!() }\n");
        }

        self.src.push_str("}\n");
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

    fn rustdoc_params(&mut self, docs: &[(String, Type)], header: &str) {
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

    fn print_signature(&mut self, func: &Function, params_owned: bool, sig: &FnSig) -> Vec<String> {
        let params = self.print_docs_and_params(func, params_owned, sig);
        if let FunctionKind::Constructor(_) = &func.kind {
            uwrite!(
                self.src,
                " -> {anyhow}::Result<Self> where Self: Sized",
                anyhow = self.gen.anyhow_path()
            );
        } else {
            self.print_results(&func.results);
        }
        params
    }

    fn print_docs_and_params(
        &mut self,
        func: &Function,
        params_owned: bool,
        sig: &FnSig,
    ) -> Vec<String> {
        self.rustdoc(&func.docs);
        self.rustdoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.rustdoc_params(&func.results, "Return");

        if !sig.private {
            self.push_str("pub ");
        }
        if !sig.definition {
            // This exists just to work around compiler bug https://github.com/rust-lang/rust/issues/100013
            self.push_str("async ");
        }
        self.push_str("fn ");
        let func_name = if sig.use_item_name {
            if let FunctionKind::Constructor(_) = &func.kind {
                "new"
            } else {
                func.item_name()
            }
        } else {
            &func.name
        };
        self.push_str(&to_rust_ident(func_name));
        self.push_str("(");
        if self.in_import {
            if let Some(arg) = &sig.self_arg {
                self.push_str(arg);
                self.push_str(",");
            }
            self.push_str("wrpc__: &impl ");
            let wrpc_transport = self.gen.wrpc_transport_path().to_string();
            self.push_str(&wrpc_transport);
            self.push_str("::Client,");
        } else {
            if let Some(arg) = &sig.self_arg {
                self.push_str(arg);
                self.push_str(",");
            }
            self.push_str("cx: Ctx,");
        }
        let mut params = Vec::new();
        for (i, (name, param)) in func.params.iter().enumerate() {
            if let FunctionKind::Method(..) = &func.kind {
                if i == 0 && sig.self_is_first_param {
                    params.push("self".to_string());
                    continue;
                }
            }
            let name = to_rust_ident(name);
            self.push_str(&name);
            self.push_str(": ");

            // Select the "style" of mode that the parameter's type will be
            // rendered as. Owned parameters are always owned, that's the easy
            // case. Otherwise it means that we're rendering the arguments to an
            // imported function which technically don't need ownership. In this
            // case the `ownership` configuration is consulted.
            //
            // If `Owning` is specified then that means that the top-level
            // argument will be `&T` but everything under that will be `T`. For
            // example a record-of-lists would be passed as `&RecordOfLists` as
            // opposed to `RecordOfLists<'a>`.
            //
            // In the `Borrowing` mode however a different tradeoff is made. The
            // types are generated differently meaning that a borrowed version
            // is used.
            let style = if params_owned {
                TypeOwnershipStyle::Owned
            } else {
                match self.gen.opts.ownership {
                    Ownership::Owning => TypeOwnershipStyle::OnlyTopBorrowed,
                    Ownership::Borrowing { .. } => TypeOwnershipStyle::Borrowed,
                }
            };
            let mode = self.type_mode_for(param, style, "'_");
            self.print_ty(param, mode);
            self.push_str(",");

            // Depending on the style of this request vs what we got perhaps
            // change how this argument is used.
            //
            // If the `mode` that was selected matches the requested style, then
            // everything is as expected and the argument should be used as-is.
            // If it differs though then that means that we requested a borrowed
            // mode but a different mode ended up being selected. This situation
            // indicates for example that an argument to an import should be
            // borrowed but the argument's type means that it can't be borrowed.
            // For example all arguments to imports are borrowed by default but
            // owned resources cannot ever be borrowed, so they pop out here as
            // owned instead.
            //
            // In such a situation the lower code still expects to be operating
            // over borrows. For example raw pointers from lists are passed to
            // the canonical ABI layer assuming that the lists are "rooted" by
            // the caller. To uphold this invariant a borrow of the argument is
            // recorded as the name of this parameter. That ensures that all
            // access to the parameter is done indirectly which pretends, at
            // least internally, that the argument was borrowed. The original
            // motivation for this was #817.
            if mode.style == style {
                params.push(name);
            } else {
                assert!(style != TypeOwnershipStyle::Owned);
                params.push(format!("&{name}"));
            }
        }
        self.push_str(")");
        params
    }

    fn print_results(&mut self, results: &Results) {
        match results.len() {
            0 => {
                uwrite!(
                    self.src,
                    " -> {anyhow}::Result<()>",
                    anyhow = self.gen.anyhow_path()
                );
            }
            1 => {
                uwrite!(
                    self.src,
                    " -> {anyhow}::Result<",
                    anyhow = self.gen.anyhow_path()
                );
                let ty = results.iter_types().next().unwrap();
                let mode = self.type_mode_for(ty, TypeOwnershipStyle::Owned, "'INVALID");
                assert!(mode.lifetime.is_none());
                self.print_ty(ty, mode);
                self.push_str(">");
            }
            _ => {
                uwrite!(
                    self.src,
                    " -> {anyhow}::Result<(",
                    anyhow = self.gen.anyhow_path()
                );
                for ty in results.iter_types() {
                    let mode = self.type_mode_for(ty, TypeOwnershipStyle::Owned, "'INVALID");
                    assert!(mode.lifetime.is_none());
                    self.print_ty(ty, mode);
                    self.push_str(", ");
                }
                self.push_str(")>");
            }
        }
    }

    /// Calculates the `TypeMode` to be used for the `ty` specified.
    ///
    /// This takes a `style` argument which is the requested style of ownership
    /// for this type. Note that the returned `TypeMode` may have a different
    /// `style`.
    ///
    /// This additionally takes a `lt` parameter which, if needed, is what will
    /// be used to render lifetimes.
    fn type_mode_for(&self, ty: &Type, style: TypeOwnershipStyle, lt: &'static str) -> TypeMode {
        match ty {
            Type::Id(id) => self.type_mode_for_id(*id, style, lt),

            // Borrowed strings are handled specially here since they're the
            // only list-like primitive.
            Type::String if style != TypeOwnershipStyle::Owned => TypeMode {
                lifetime: Some(lt),
                lists_borrowed: true,
                style,
            },

            _ => TypeMode::owned(),
        }
    }

    /// Same as `type_mode_for`, but specifically for `TypeId` which refers to a
    /// type.
    fn type_mode_for_id(
        &self,
        ty: TypeId,
        style: TypeOwnershipStyle,
        lt: &'static str,
    ) -> TypeMode {
        // NB: This method is the heart of determining how to render types.
        // There's a lot of permutations and corner cases to handle, especially
        // with being able to configure at the generator level how types are
        // generated. Long story short this is a subtle and complicated method.
        //
        // The hope is that most of the complexity around type generation in
        // Rust is largely centered here where everything else can lean on this.
        // This has gone through so many refactors I've lost count at this
        // point, but maybe this one is the one that'll stick!
        //
        // The general idea is that there's some clear-and-fast rules for how
        // `TypeMode` must be returned here. For example borrowed handles are
        // required to have a lifetime parameter. Everything else though is here
        // to handle the various levels of configuration and semantics for each
        // level of types.
        //
        // As a reminder a `TypeMode` is generated for each "level" of a type
        // hierarchy, for example there's one mode for `Vec<T>` and another mode
        // for `T`. This enables, for example, rendering the outer layer as
        // either `Vec<T>` or `&[T]` but the inner `T` may or may not have a
        // type parameter.

        let info = self.info(ty);
        let lifetime = if info.has_borrow_handle {
            // Borrowed handles always have a lifetime associated with them so
            // thread it through.
            Some(lt)
        } else if style == TypeOwnershipStyle::Owned {
            // If this type is being rendered as an "owned" type, and it
            // doesn't have any borrowed handles, then no lifetimes are needed
            // since any internal lists will be their owned version.
            None
        } else if info.has_own_handle || !info.has_list {
            // At this point there are no borrowed handles and a borrowed style
            // of type is requested. In this situation there's two cases where a
            // lifetime is never used:
            //
            // * Owned handles are present - in this situation ownership is used
            //   to statically reflect how a value is lost when passed to an
            //   import. This means that no lifetime is used for internal lists
            //   since they must be rendered in an owned mode.
            //
            // * There are no lists present - here the lifetime parameter won't
            //   be used for anything because there's no borrows or lists, so
            //   it's skipped.
            None
        } else if !info.owned || self.uses_two_names(&info) {
            // This next layer things get a little more interesting. To recap,
            // so far we know that there's no borrowed handles, a borrowed mode
            // is requested, there's no own handles, and there's a list. In that
            // situation if `info` shows that this type is never used in an
            // owned position, or if two types are explicitly requested for
            // owned/borrowed values, then a lifetime is used.
            Some(lt)
        } else {
            // ... and finally, here at the end we know:
            //
            // * No borrowed handles
            // * Borrowing mode is requested
            // * No owned handles
            // * A list is somewhere
            // * This type is used somewhere in an owned position
            // * This type does not used "two names" meaning that we must use
            //   the owned version of the type.
            //
            // If the configured ownership mode for generating types of this
            // generator is "owning" then that's the only type that can be used.
            // If borrowing is requested then this means that `&T` is going to
            // be rendered, so thread it through.
            //
            // If the configured ownership mode uses borrowing by default, then
            // things get a little weird. This means that a lifetime is going to
            // be used an any lists should be borrowed, but we specifically
            // switch to only borrowing the top layer of the type rather than
            // the entire hierarchy. This situation can happen in
            // `duplicate_if_necessary: false` mode for example where we're
            // borrowing a type which is used in an owned position elsewhere.
            // The only possibility at that point is to borrow it at the root
            // but everything else internally is required to be owned from then
            // on.
            match self.gen.opts.ownership {
                Ownership::Owning => Some(lt),
                Ownership::Borrowing { .. } => {
                    return TypeMode {
                        lifetime: Some(lt),
                        lists_borrowed: true,
                        style: TypeOwnershipStyle::OnlyTopBorrowed,
                    };
                }
            }
        };
        TypeMode {
            lifetime,

            // If a lifetime is present and ownership isn't requested, then make
            // sure any lists show up as `&str` or `&[T]`.
            lists_borrowed: lifetime.is_some() && style != TypeOwnershipStyle::Owned,

            // Switch the style to `Owned` if an `own<T>` handle is present
            // because there's no option but to take interior types by ownership
            // as that statically shows that the ownership of the value is being
            // lost.
            style: if info.has_own_handle {
                TypeOwnershipStyle::Owned
            } else {
                style
            },
        }
    }

    /// Generates the "next" mode for a type.
    ///
    /// The `ty` specified is the type that a mode is being generated for, and
    /// the `mode` argument is the "parent" mode that the previous outer layer
    /// of type was rendered with. The returned mode should be used to render
    /// `ty`.
    fn filter_mode(&self, ty: &Type, mode: TypeMode) -> TypeMode {
        match mode.lifetime {
            Some(lt) => self.type_mode_for(ty, mode.style.next(), lt),
            None => TypeMode::owned(),
        }
    }

    /// Same as `filder_mode` except if `mode` has the type `OnlyTopBorrowed`
    /// the `mode` is specifically preserved as-is.
    ///
    /// This is used for types like `Option<T>` to render as `Option<&T>`
    /// instead of `&Option<T>` for example.
    fn filter_mode_preserve_top(&self, ty: &Type, mode: TypeMode) -> TypeMode {
        if mode.style == TypeOwnershipStyle::OnlyTopBorrowed {
            mode
        } else {
            self.filter_mode(ty, mode)
        }
    }

    fn print_ty(&mut self, ty: &Type, mode: TypeMode) {
        match ty {
            Type::Id(t) => self.print_tyid(*t, mode),
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
                assert_eq!(mode.lists_borrowed, mode.lifetime.is_some());
                match mode.lifetime {
                    Some(lt) => self.print_borrowed_str(lt),
                    None => {
                        if self.gen.opts.raw_strings {
                            self.push_str("Vec<u8>");
                        } else {
                            self.push_str("String");
                        }
                    }
                }
            }
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>, mode: TypeMode) {
        match ty {
            Some(ty) => {
                let mode = self.filter_mode_preserve_top(ty, mode);
                self.print_ty(ty, mode);
            }
            None => self.push_str("()"),
        }
    }

    pub fn type_path(&self, id: TypeId, owned: bool) -> String {
        self.type_path_with_name(
            id,
            if owned {
                self.result_name(id)
            } else {
                self.param_name(id)
            },
        )
    }

    fn type_path_with_name(&self, id: TypeId, name: String) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner {
            if let Some(path) = self.path_to_interface(id) {
                return format!("{path}::{name}");
            }
        }
        name
    }

    fn print_tyid(&mut self, id: TypeId, mode: TypeMode) {
        let ty = &self.resolve.types[id];
        if ty.name.is_some() {
            // NB: Most of the heavy lifting of `TypeMode` and what to do here
            // has already happened in `type_mode_for*`. Here though a little
            // more happens because this is where `OnlyTopBorrowed` is
            // processed.
            //
            // Specifically what should happen is that in the case of an
            // argument to an imported function if only the top value is
            // borrowed then we want to render it as `&T`. If this all is
            // applicable then the lifetime is rendered here before the type.
            // The `mode` is then switched to `Owned` and recalculated for the
            // type we're rendering here to avoid accidentally giving it a
            // lifetime type parameter when it otherwise doesn't have it.
            let mode = if mode.style == TypeOwnershipStyle::OnlyTopBorrowed {
                if let Some(lt) = mode.lifetime {
                    self.push_str("&");
                    if lt != "'_" {
                        self.push_str(lt);
                        self.push_str(" ");
                    }
                    self.type_mode_for_id(id, TypeOwnershipStyle::Owned, lt)
                } else {
                    mode
                }
            } else {
                mode
            };
            let name = self.type_path(
                id,
                match mode.style {
                    TypeOwnershipStyle::Owned => true,
                    TypeOwnershipStyle::OnlyTopBorrowed | TypeOwnershipStyle::Borrowed => false,
                },
            );
            self.push_str(&name);
            return;
        }

        match &ty.kind {
            TypeDefKind::List(t) => self.print_list(t, mode),

            TypeDefKind::Option(t) => {
                self.push_str("Option<");
                let mode = self.filter_mode_preserve_top(t, mode);
                self.print_ty(t, mode);
                self.push_str(">");
            }

            TypeDefKind::Result(r) => {
                self.push_str("Result<");
                self.print_optional_ty(r.ok.as_ref(), mode);
                self.push_str(",");
                self.print_optional_ty(r.err.as_ref(), mode);
                self.push_str(">");
            }

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            // Tuple-like records are mapped directly to Rust tuples of
            // types. Note the trailing comma after each member to
            // appropriately handle 1-tuples.
            TypeDefKind::Tuple(t) => {
                self.push_str("(");
                for ty in &t.types {
                    let mode = self.filter_mode_preserve_top(ty, mode);
                    self.print_ty(ty, mode);
                    self.push_str(",");
                }
                self.push_str(")");
            }
            TypeDefKind::Resource => {
                panic!("unsupported anonymous type reference: resource")
            }
            TypeDefKind::Record(_) => {
                panic!("unsupported anonymous type reference: record")
            }
            TypeDefKind::Flags(_) => {
                panic!("unsupported anonymous type reference: flags")
            }
            TypeDefKind::Enum(_) => {
                panic!("unsupported anonymous type reference: enum")
            }
            TypeDefKind::Future(ty) => {
                self.push_str("Future<");
                self.print_optional_ty(ty.as_ref(), mode);
                self.push_str(">");
            }
            TypeDefKind::Stream(stream) => {
                self.push_str("Stream<");
                self.print_optional_ty(stream.element.as_ref(), mode);
                self.push_str(",");
                self.print_optional_ty(stream.end.as_ref(), mode);
                self.push_str(">");
            }

            TypeDefKind::Handle(Handle::Own(ty)) => {
                self.print_ty(&Type::Id(*ty), mode);
            }

            TypeDefKind::Handle(Handle::Borrow(ty)) => {
                if self.is_exported_resource(*ty) {
                    let camel = self.resolve.types[*ty]
                        .name
                        .as_deref()
                        .unwrap()
                        .to_upper_camel_case();
                    let name = format!("{camel}Borrow");
                    self.push_str(&self.type_path_with_name(*ty, name));
                } else {
                    let ty = &Type::Id(*ty);
                    let mode = self.filter_mode(ty, mode);
                    self.print_ty(ty, mode);
                }
            }

            TypeDefKind::Type(t) => self.print_ty(t, mode),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_noop_subscribe(&mut self, name: &str) {
        uwrite!(
            self.src,
            "impl {wrpc_transport}::Subscribe for {name} {{}}",
            wrpc_transport = self.gen.wrpc_transport_path()
        );
    }

    fn print_list(&mut self, ty: &Type, mode: TypeMode) {
        let next_mode = self.filter_mode(ty, mode);
        if mode.lists_borrowed {
            let lifetime = mode.lifetime.unwrap();
            self.push_str("&");
            if lifetime != "'_" {
                self.push_str(lifetime);
                self.push_str(" ");
            }
            self.push_str("[");
            self.print_ty(ty, next_mode);
            self.push_str("]");
        } else {
            self.push_str("Vec<");
            self.print_ty(ty, next_mode);
            self.push_str(">");
        }
    }

    fn print_struct_encode(&mut self, name: &str, ty: &Record) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl");
        uwrite!(self.src, " {wrpc_transport}::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "#[allow(unused_mut)]");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{");
        if !ty.fields.is_empty() {
            uwriteln!(self.src, "use {anyhow}::Context as _;");
            uwriteln!(self.src, r#"let span = {tracing}::trace_span!("encode");"#);
            self.push_str("let _enter = span.enter();\n");
            self.push_str("let ");
            self.push_str(name.trim_start_matches('&'));
            self.push_str("{\n");
            for Field { name, .. } in &ty.fields {
                let name_rust = to_rust_ident(name);
                uwriteln!(self.src, "{name_rust}: f_{name_rust},");
            }
            self.push_str("} = self;\n");
            for Field { name, .. } in &ty.fields {
                let name_rust = to_rust_ident(name);
                uwriteln!(
                    self.src,
                    r#"let f_{name_rust} = f_{name_rust}.encode(&mut payload).await.context("failed to encode `{name}`")?;"#,
                );
            }
            self.push_str("if false");
            for Field { name, .. } in &ty.fields {
                uwrite!(self.src, r#" || f_{}.is_some()"#, to_rust_ident(name));
            }
            self.push_str("{\n");
            uwrite!(
                self.src,
                "return Ok(Some({wrpc_transport}::AsyncValue::Record(vec!["
            );
            for Field { name, .. } in &ty.fields {
                uwrite!(self.src, r#"f_{},"#, to_rust_ident(name));
            }
            self.push_str("\n])))}\n");
        }
        self.push_str("Ok(None)\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_struct_subscribe(&mut self, name: &str, ty: &Record) {
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        self.push_str("#[automatically_derived]\n");
        self.push_str("impl");
        uwrite!(self.src, " {wrpc_transport}::Subscribe for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "async fn subscribe<T: {wrpc_transport}::Subscriber + Send + Sync>(subscriber: &T, subject: T::Subject) -> Result<Option<{wrpc_transport}::AsyncSubscription<T::Stream>>, T::SubscribeError> {{");
        if !ty.fields.is_empty() {
            self.push_str("#[allow(unused_imports)]\n");
            uwriteln!(self.src, "use {wrpc_transport}::Subject as _;");
            uwriteln!(
                self.src,
                r#"let span = {tracing}::trace_span!("subscribe");"#
            );
            self.push_str("let _enter = span.enter();\n");
            for (i, Field { name, ty, .. }) in ty.fields.iter().enumerate() {
                let name_rust = to_rust_ident(name);
                uwrite!(self.src, r#"let f_{name_rust} = <"#,);
                self.print_ty(ty, TypeMode::owned());
                uwrite!(
                    self.src,
                    r#">::subscribe(subscriber, subject.child(Some({i}))).await?;"#,
                );
            }
            self.push_str("if false");
            for Field { name, .. } in &ty.fields {
                uwrite!(self.src, r#" || f_{}.is_some()"#, to_rust_ident(name));
            }
            self.push_str("{\n");
            uwrite!(
                self.src,
                "return Ok(Some({wrpc_transport}::AsyncSubscription::Record(vec!["
            );
            for Field { name, .. } in &ty.fields {
                uwrite!(self.src, r#"f_{},"#, to_rust_ident(name));
            }
            self.push_str("\n])))}\n");
        }
        self.push_str("Ok(None)\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_struct_receive(&mut self, name: &str, ty: &Record) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let futures = self.gen.futures_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " {wrpc_transport}::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        self.push_str(" {");
        uwriteln!(
            self.src,
            r#"
    #[allow(unused_mut)]
    async fn receive<T>(
        payload: impl {bytes}::Buf + Send + 'wrpc_receive_,
        mut rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
        sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
    ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'wrpc_receive_>)>
        where
            T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static {{"#
        );
        if !ty.fields.is_empty() {
            uwriteln!(self.src, "use {anyhow}::Context as _;");
            uwriteln!(self.src, r#"let span = {tracing}::trace_span!("receive");"#);
            self.push_str("let _enter = span.enter();\n");
            uwriteln!(self.src, "let mut sub = sub.map({wrpc_transport}::AsyncSubscription::try_unwrap_record).transpose()?;");
            for (i, Field { name, .. }) in ty.fields.iter().enumerate() {
                let name_rust = to_rust_ident(name);
                uwriteln!(
                    self.src,
                    r#"let (f_{name_rust}, payload) = {wrpc_transport}::Receive::receive(
                        payload,
                        &mut rx,
                        sub.as_mut().and_then(|sub| sub.get_mut({i})).and_then(Option::take)
                    )
                    .await
                    .context("failed to receive `{name}`")?;"#,
                );
            }
        }
        self.push_str("Ok((Self {\n");
        for Field { name, .. } in &ty.fields {
            let name_rust = to_rust_ident(name);
            uwrite!(self.src, r#"{name_rust}: f_{name_rust},"#);
        }
        self.push_str("\n}, payload))\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_variant_encode<'a>(
        &mut self,
        name: &str,
        cases: impl IntoIterator<Item = (String, &'a Docs, Option<&'a Type>)>,
    ) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl");
        uwrite!(self.src, " {wrpc_transport}::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "#[allow(unused_mut)]");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{");
        uwriteln!(self.src, r#"let span = {tracing}::trace_span!("encode");"#);
        self.push_str("let _enter = span.enter();\n");
        self.push_str("match self {\n");
        for (i, (case_name, _, payload)) in cases.into_iter().enumerate() {
            self.push_str(name.trim_start_matches('&'));
            self.push_str("::");
            self.push_str(&case_name);
            if payload.is_some() {
                self.push_str("(nested)");
            }
            self.push_str(" => {\n");
            uwriteln!(
                self.src,
                "{wrpc_transport}::encode_discriminant(&mut payload, {i})?;"
            );
            if payload.is_some() {
                self.push_str("nested.encode(payload).await\n");
            } else {
                self.push_str("Ok(None)\n");
            }
            self.push_str("}\n");
        }
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_variant_subscribe<'a>(
        &mut self,
        name: &str,
        cases: impl IntoIterator<Item = (String, &'a Docs, Option<&'a Type>)> + Clone,
    ) {
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        self.push_str("#[automatically_derived]\n");
        self.push_str("impl");
        uwrite!(self.src, " {wrpc_transport}::Subscribe for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "async fn subscribe<T: {wrpc_transport}::Subscriber + Send + Sync>(subscriber: &T, subject: T::Subject) -> Result<Option<{wrpc_transport}::AsyncSubscription<T::Stream>>, T::SubscribeError> {{");
        self.push_str("#[allow(unused_imports)]\n");
        uwriteln!(self.src, "use {wrpc_transport}::Subject as _;");
        uwriteln!(
            self.src,
            r#"let span = {tracing}::trace_span!("subscribe");"#
        );
        self.push_str("let _enter = span.enter();\n");
        for (i, (_, _, payload)) in cases.clone().into_iter().enumerate() {
            if let Some(ty) = payload {
                uwrite!(self.src, r#"let c_{i} = <"#,);
                self.print_ty(ty, TypeMode::owned());
                uwrite!(
                    self.src,
                    r#">::subscribe(subscriber, subject.child(Some({i}))).await?;"#,
                );
            }
        }
        self.push_str("if false");
        for (i, (_, _, payload)) in cases.clone().into_iter().enumerate() {
            if payload.is_some() {
                uwrite!(self.src, r#" || c_{i}.is_some()"#);
            }
        }
        self.push_str("{\n");
        uwrite!(
            self.src,
            "return Ok(Some({wrpc_transport}::AsyncSubscription::Variant(vec!["
        );
        for (i, (_, _, payload)) in cases.into_iter().enumerate() {
            if payload.is_some() {
                uwrite!(self.src, r#"c_{i},"#);
            } else {
                self.push_str("None,");
            }
        }
        self.push_str("\n])))}\n");
        self.push_str("Ok(None)\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_variant_receive<'a>(
        &mut self,
        name: &str,
        cases: impl IntoIterator<Item = (String, &'a Docs, Option<&'a Type>)>,
    ) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let futures = self.gen.futures_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " {wrpc_transport}::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        uwriteln!(
            self.src,
            r#" {{
        async fn receive<T>(
            payload: impl {bytes}::Buf + Send + 'wrpc_receive_,
            mut rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
            sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
        ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'wrpc_receive_>)>
            where
                T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
            {{
                use {anyhow}::Context as _;
                let span = {tracing}::trace_span!("receive");
                let _enter = span.enter();
                let (disc, payload) = {wrpc_transport}::receive_discriminant(
                        payload,
                        &mut rx,
                    )
                    .await
                    .context("failed to receive discriminant")?;
                match disc {{"#,
        );
        for (i, (case_name, _, payload)) in cases.into_iter().enumerate() {
            uwriteln!(self.src, "{i} => ");
            if payload.is_some() {
                uwriteln!(
                    self.src,
                    r#"{{
                        let sub = sub
                            .map(|sub| {{
                                let mut sub = sub.try_unwrap_variant()?;
                                {anyhow}::Ok(sub.get_mut({i}).and_then(Option::take))
                            }})
                            .transpose()?
                            .flatten();
                        let (nested, payload) = {wrpc_transport}::Receive::receive(payload, rx, sub)
                            .await
                            .context("failed to receive nested variant value")?;
                        Ok((Self::{case_name}(nested), payload))
                    }}"#
                );
            } else {
                uwriteln!(self.src, "Ok((Self::{case_name}, payload)),");
            }
        }
        uwriteln!(
            self.src,
            r#"_ => {anyhow}::bail!("unknown variant discriminant `{{disc}}`")"#
        );
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_enum_encode(&mut self, name: &str, cases: &[EnumCase]) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl");
        uwrite!(self.src, " {wrpc_transport}::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "#[allow(unused_mut)]");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{");
        uwriteln!(self.src, r#"let span = {tracing}::trace_span!("encode");"#);
        self.push_str("let _enter = span.enter();\n");
        self.push_str("match self {\n");
        for (i, case) in cases.iter().enumerate() {
            let case = case.name.to_upper_camel_case();
            self.push_str(name.trim_start_matches('&'));
            self.push_str("::");
            self.push_str(&case);
            self.push_str(" => {\n");
            uwriteln!(
                self.src,
                "{wrpc_transport}::encode_discriminant(&mut payload, {i})?;"
            );
            self.push_str("Ok(None)\n");
            self.push_str("}\n");
        }
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_enum_receive(&mut self, name: &str, cases: &[EnumCase]) {
        let anyhow = self.gen.anyhow_path().to_string();
        let async_trait = self.gen.async_trait_path().to_string();
        let bytes = self.gen.bytes_path().to_string();
        let futures = self.gen.futures_path().to_string();
        let tracing = self.gen.tracing_path().to_string();
        let wrpc_transport = self.gen.wrpc_transport_path().to_string();

        uwriteln!(self.src, "#[{async_trait}::async_trait]");
        self.push_str("#[automatically_derived]\n");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " {wrpc_transport}::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        uwriteln!(
            self.src,
            r#" {{
        async fn receive<T>(
            payload: impl {bytes}::Buf + Send + 'wrpc_receive_,
            mut rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
            sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
        ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'wrpc_receive_>)>
            where
                T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
            {{
                use {anyhow}::Context as _;
                let span = {tracing}::trace_span!("receive");
                let _enter = span.enter();
                let (disc, payload) = {wrpc_transport}::receive_discriminant(
                        payload,
                        &mut rx,
                    )
                    .await
                    .context("failed to receive discriminant")?;
                match disc {{"#,
        );
        for (i, case) in cases.iter().enumerate() {
            let case = case.name.to_upper_camel_case();
            uwriteln!(self.src, "{i} => Ok((Self::{case}, payload)),");
        }
        uwriteln!(
            self.src,
            r#"_ => {anyhow}::bail!("unknown enum discriminant `{{disc}}`")"#
        );
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn int_repr(&mut self, repr: Int) {
        self.push_str(int_repr(repr));
    }

    fn modes_of(&self, ty: TypeId) -> Vec<(String, TypeMode)> {
        let info = self.info(ty);
        let mut result = Vec::new();
        if !self.gen.opts.generate_unused_types {
            // If this type isn't actually used, no need to generate it.
            if !info.owned && !info.borrowed {
                return result;
            }
        }
        // Generate one mode for when the type is owned and another for when
        // it's borrowed.
        let a = self.type_mode_for_id(ty, TypeOwnershipStyle::Owned, "'_");
        let b = self.type_mode_for_id(ty, TypeOwnershipStyle::Borrowed, "'_");

        if self.uses_two_names(&info) {
            // If this type uses two names then, well, it uses two names. In
            // this situation both modes are returned.
            assert!(a != b);
            result.push((self.result_name(ty), a));
            result.push((self.param_name(ty), b));
        } else if a == b {
            // If the modes are the same then there's only one result.
            result.push((self.result_name(ty), a));
        } else if info.owned || matches!(self.gen.opts.ownership, Ownership::Owning) {
            // If this type is owned or if ownership is preferred then the owned
            // variant is used as a priority. This is where the generator's
            // configuration comes into play.
            result.push((self.result_name(ty), a));
        } else {
            // And finally, failing all that, the borrowed variant is used.
            assert!(!info.owned);
            result.push((self.param_name(ty), b));
        }
        result
    }

    fn print_typedef_record(&mut self, id: TypeId, record: &Record, docs: &Docs) {
        let info = self.info(id);
        // We use a BTree set to make sure we don't have any duplicates and we have a stable order
        let additional_derives: BTreeSet<String> = self
            .gen
            .opts
            .additional_derive_attributes
            .iter()
            .cloned()
            .collect();
        for (name, _) in self.modes_of(id) {
            self.rustdoc(docs);
            let mut derives = additional_derives.clone();
            if info.is_copy() {
                self.push_str("#[repr(C)]\n");
                derives.extend(
                    ["Copy", "Clone"]
                        .into_iter()
                        .map(std::string::ToString::to_string),
                );
            } else if info.is_clone() {
                derives.insert("Clone".to_string());
            }
            if !derives.is_empty() {
                self.push_str("#[derive(");
                self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
                self.push_str(")]\n");
            }
            self.push_str(&format!("pub struct {name}"));
            self.push_str(" {\n");
            for Field { name, ty, docs } in &record.fields {
                self.rustdoc(docs);
                self.push_str("pub ");
                self.push_str(&to_rust_ident(name));
                self.push_str(": ");
                self.print_ty(ty, TypeMode::owned());
                self.push_str(",\n");
            }
            self.push_str("}\n");

            self.print_struct_encode(&name, record);
            self.print_struct_encode(&format!("&{name}"), record);

            self.print_struct_subscribe(&name, record);
            self.print_struct_receive(&name, record);

            self.push_str("impl");
            self.push_str(" ::core::fmt::Debug for ");
            self.push_str(&name);
            self.push_str(" {\n");
            self.push_str(
                "fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {\n",
            );
            self.push_str(&format!("f.debug_struct(\"{name}\")"));
            for field in &record.fields {
                self.push_str(&format!(
                    ".field(\"{}\", &self.{})",
                    field.name,
                    to_rust_ident(&field.name)
                ));
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
                if self.gen.opts.std_feature {
                    self.push_str("#[cfg(feature = \"std\")]\n");
                }
                self.push_str("impl std::error::Error for ");
                self.push_str(&name);
                self.push_str(" {}\n");
            }
        }
    }

    fn print_typedef_variant(&mut self, id: TypeId, variant: &Variant, docs: &Docs)
    where
        Self: Sized,
    {
        self.print_rust_enum(
            id,
            variant
                .cases
                .iter()
                .map(|c| (c.name.to_upper_camel_case(), &c.docs, c.ty.as_ref())),
            docs,
        );
    }

    fn print_rust_enum<'b>(
        &mut self,
        id: TypeId,
        cases: impl IntoIterator<Item = (String, &'b Docs, Option<&'b Type>)> + Clone,
        docs: &Docs,
    ) where
        Self: Sized,
    {
        let info = self.info(id);
        // We use a BTree set to make sure we don't have any duplicates and have a stable order
        let additional_derives: BTreeSet<String> = self
            .gen
            .opts
            .additional_derive_attributes
            .iter()
            .cloned()
            .collect();
        for (name, _) in self.modes_of(id) {
            self.rustdoc(docs);
            let mut derives = additional_derives.clone();
            if info.is_copy() {
                derives.extend(
                    ["Copy", "Clone"]
                        .into_iter()
                        .map(std::string::ToString::to_string),
                );
            } else if info.is_clone() {
                derives.insert("Clone".to_string());
            }
            if !derives.is_empty() {
                self.push_str("#[derive(");
                self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
                self.push_str(")]\n");
            }
            self.push_str(&format!("pub enum {name}"));
            self.push_str(" {\n");
            for (case_name, docs, payload) in cases.clone() {
                self.rustdoc(docs);
                self.push_str(&case_name);
                if let Some(ty) = payload {
                    self.push_str("(");
                    self.print_ty(ty, TypeMode::owned());
                    self.push_str(")");
                }
                self.push_str(",\n");
            }
            self.push_str("}\n");

            self.print_rust_enum_debug(
                &name,
                cases
                    .clone()
                    .into_iter()
                    .map(|(name, _docs, ty)| (name, ty)),
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

                if self.gen.opts.std_feature {
                    self.push_str("#[cfg(feature = \"std\")]\n");
                }
                self.push_str("impl");
                self.push_str(" std::error::Error for ");
                self.push_str(&name);
                self.push_str(" {}\n");
            }

            self.print_variant_encode(&name, cases.clone());
            self.print_variant_encode(&format!("&{name}"), cases.clone());

            self.print_variant_subscribe(&name, cases.clone());

            self.print_variant_receive(&name, cases.clone());
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
            if payload.is_some() {
                self.push_str("(e)");
            }
            self.push_str(" => {\n");
            self.push_str(&format!("f.debug_tuple(\"{name}::{case_name}\")"));
            if payload.is_some() {
                self.push_str(".field(e)");
            }
            self.push_str(".finish()\n");
            self.push_str("}\n");
        }
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_typedef_option(&mut self, id: TypeId, payload: &Type, docs: &Docs) {
        for (name, mode) in self.modes_of(id) {
            self.rustdoc(docs);
            self.push_str(&format!("pub type {name}"));
            self.push_str("= Option<");
            self.print_ty(payload, mode);
            self.push_str(">;\n");
        }
    }

    fn print_typedef_result(&mut self, id: TypeId, result: &Result_, docs: &Docs) {
        for (name, mode) in self.modes_of(id) {
            self.rustdoc(docs);
            self.push_str(&format!("pub type {name}"));
            self.push_str("= Result<");
            self.print_optional_ty(result.ok.as_ref(), mode);
            self.push_str(",");
            self.print_optional_ty(result.err.as_ref(), mode);
            self.push_str(">;\n");
        }
    }

    fn print_typedef_enum(
        &mut self,
        id: TypeId,
        name: &str,
        enum_: &Enum,
        docs: &Docs,
        attrs: &[String],
        case_attr: Box<dyn Fn(&EnumCase) -> String>,
    ) where
        Self: Sized,
    {
        let info = self.info(id);

        let name = to_upper_camel_case(name);
        self.rustdoc(docs);
        for attr in attrs {
            self.push_str(&format!("{attr}\n"));
        }
        self.push_str("#[repr(");
        self.int_repr(enum_.tag());
        self.push_str(")]\n");
        // We use a BTree set to make sure we don't have any duplicates and a stable order
        let mut derives: BTreeSet<String> = self
            .gen
            .opts
            .additional_derive_attributes
            .iter()
            .cloned()
            .collect();
        derives.extend(
            ["Clone", "Copy", "PartialEq", "Eq"]
                .into_iter()
                .map(std::string::ToString::to_string),
        );
        self.push_str("#[derive(");
        self.push_str(&derives.into_iter().collect::<Vec<_>>().join(", "));
        self.push_str(")]\n");
        self.push_str(&format!("pub enum {name} {{\n"));
        for case in &enum_.cases {
            self.rustdoc(&case.docs);
            self.push_str(&case_attr(case));
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
            if self.gen.opts.std_feature {
                self.push_str("#[cfg(feature = \"std\")]\n");
            }
            self.push_str("impl std::error::Error for ");
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
    }

    fn print_typedef_alias(&mut self, id: TypeId, ty: &Type, docs: &Docs) {
        for (name, mode) in self.modes_of(id) {
            self.rustdoc(docs);
            self.push_str(&format!("pub type {name}"));
            self.push_str(" = ");
            self.print_ty(ty, mode);
            self.push_str(";\n");
        }

        if self.is_exported_resource(id) {
            self.rustdoc(docs);
            let name = self.resolve.types[id].name.as_ref().unwrap();
            let name = name.to_upper_camel_case();
            self.push_str(&format!("pub type {name}Borrow"));
            self.push_str(" = ");
            self.print_ty(ty, TypeMode::owned());
            self.push_str("Borrow");
            self.push_str(";\n");
        }
    }

    fn param_name(&self, ty: TypeId) -> String {
        let info = self.info(ty);
        let name = to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap());
        if self.uses_two_names(&info) {
            format!("{name}Param")
        } else {
            name
        }
    }

    fn result_name(&self, ty: TypeId) -> String {
        let info = self.info(ty);
        let name = to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap());
        if self.uses_two_names(&info) {
            format!("{name}Result")
        } else {
            name
        }
    }

    fn uses_two_names(&self, info: &TypeInfo) -> bool {
        // Types are only duplicated if explicitly requested ...
        matches!(
            self.gen.opts.ownership,
            Ownership::Borrowing {
                duplicate_if_necessary: true
            }
        )
            // ... and if they're both used in a borrowed/owned context
            && info.borrowed
            && info.owned
            // ... and they have a list ...
            && info.has_list
            // ... and if there's NOT an `own` handle since those are always
            // done by ownership.
            && !info.has_own_handle
    }

    fn path_to_interface(&self, interface: InterfaceId) -> Option<String> {
        let InterfaceName { path, remapped } = &self.gen.interface_names[&interface];
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

    pub fn is_exported_resource(&self, ty: TypeId) -> bool {
        let ty = dealias(self.resolve, ty);
        let ty = &self.resolve.types[ty];
        match &ty.kind {
            TypeDefKind::Resource => {}
            _ => return false,
        }

        match ty.owner {
            // Worlds cannot export types of any kind as of this writing.
            TypeOwner::World(_) => false,

            // Interfaces are "stateful" currently where whatever we last saw
            // them as dictates whether it's exported or not.
            TypeOwner::Interface(i) => !self.gen.interface_last_seen_as_import[&i],

            // Shouldn't be the case for resources
            TypeOwner::None => unreachable!(),
        }
    }

    fn push_str(&mut self, s: &str) {
        self.src.push_str(s);
    }

    fn info(&self, ty: TypeId) -> TypeInfo {
        self.gen.types.get(ty)
    }

    fn print_borrowed_str(&mut self, lifetime: &'static str) {
        self.push_str("&");
        if lifetime != "'_" {
            self.push_str(lifetime);
            self.push_str(" ");
        }
        if self.gen.opts.raw_strings {
            self.push_str("[u8]");
        } else {
            self.push_str("str");
        }
    }
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(&mut self, id: TypeId, _name: &str, record: &Record, docs: &Docs) {
        self.print_typedef_record(id, record, docs);
    }

    fn type_resource(&mut self, _id: TypeId, name: &str, docs: &Docs) {
        self.rustdoc(docs);
        let camel = to_upper_camel_case(name);

        let anyhow = self.gen.anyhow_path();
        let async_trait = self.gen.async_trait_path();
        let bytes = self.gen.bytes_path();
        let futures = self.gen.futures_path();
        let tracing = self.gen.tracing_path();
        let wrpc_transport = self.gen.wrpc_transport_path();

        if self.in_import {
            uwriteln!(
                self.src,
                r#"
#[derive(Debug)]
pub struct {camel}(String);

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for {camel} {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        self.0.encode(payload).await
     }}
}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for &{camel} {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        self.0.as_str().encode(payload).await
     }}
}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for {camel} {{}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for &{camel} {{}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl<'a> {wrpc_transport}::Receive<'a> for {camel} {{
     async fn receive<T>(
        payload: impl {bytes}::Buf + Send + 'a,
        rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
        sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
     ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'a>)>
     where
         T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
     {{
        let span = {tracing}::trace_span!("receive");
        let _enter = span.enter();
        let (subject, payload) = {wrpc_transport}::Receive::receive(payload, rx, sub).await?;
        Ok((Self(subject), payload))
     }}
}}
"#
            );
        } else {
            uwriteln!(
                self.src,
                r#"
#[derive(Debug)]
#[repr(transparent)]
pub struct {camel};

impl {camel} {{
    /// Creates a new resource from the specified representation.
    pub fn new() -> Self {{ Self {{ }} }}
}}

/// A borrowed version of [`{camel}`] which represents a borrowed value
#[derive(Debug)]
#[repr(transparent)]
pub struct {camel}Borrow;

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for {camel}Borrow {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for &{camel}Borrow {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for {camel}Borrow {{}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for &{camel}Borrow {{}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl<'a> {wrpc_transport}::Receive<'a> for {camel}Borrow {{
     async fn receive<T>(
        payload: impl {bytes}::Buf + Send + 'a,
        rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
        sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
     ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'a>)>
     where
         T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
     {{
        let span = {tracing}::trace_span!("receive");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for {camel} {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl {wrpc_transport}::Encode for &{camel} {{
     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
        let span = {tracing}::trace_span!("encode");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for {camel} {{}}

#[automatically_derived]
impl {wrpc_transport}::Subscribe for &{camel} {{}}

#[{async_trait}::async_trait]
#[automatically_derived]
impl<'a> {wrpc_transport}::Receive<'a> for {camel} {{
     async fn receive<T>(
        payload: impl {bytes}::Buf + Send + 'a,
        rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
        sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
     ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'a>)>
     where
         T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
     {{
        let span = {tracing}::trace_span!("receive");
        let _enter = span.enter();
        {anyhow}::bail!("exported resources not supported yet")
     }}
}}
"#,
            );
        }
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        for (name, mode) in self.modes_of(id) {
            self.rustdoc(docs);
            self.push_str(&format!("pub type {name}"));
            self.push_str(" = (");
            for ty in &tuple.types {
                let mode = self.filter_mode(ty, mode);
                self.print_ty(ty, mode);
                self.push_str(",");
            }
            self.push_str(");\n");
        }
    }

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.src.push_str(&format!(
            "{bitflags}::bitflags! {{\n",
            bitflags = self.gen.bitflags_path()
        ));
        self.rustdoc(docs);
        let repr = RustFlagsRepr::new(flags);
        let name = to_upper_camel_case(name);
        self.src.push_str(&format!(
            "#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]\npub struct {name}: {repr} {{\n",
        ));
        for (i, flag) in flags.flags.iter().enumerate() {
            self.rustdoc(&flag.docs);
            self.src.push_str(&format!(
                "const {} = 1 << {};\n",
                flag.name.to_shouty_snake_case(),
                i,
            ));
        }
        self.src.push_str("}\n");
        self.src.push_str("}\n");

        self.print_noop_subscribe(&name);
        self.print_noop_subscribe(&format!("&{name}"));

        uwriteln!(
            self.src,
            r#"
                #[{async_trait}::async_trait]
                #[automatically_derived]
                impl {wrpc_transport}::Encode for {name} {{
                     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
                        let span = {tracing}::trace_span!("encode");
                        let _enter = span.enter();
                        self.bits().encode(payload).await
                     }}
                }}

                #[{async_trait}::async_trait]
                #[automatically_derived]
                impl {wrpc_transport}::Encode for &{name} {{
                     async fn encode(self, payload: &mut (impl {bytes}::BufMut + Send)) -> {anyhow}::Result<Option<{wrpc_transport}::AsyncValue>> {{
                        let span = {tracing}::trace_span!("encode");
                        let _enter = span.enter();
                        self.bits().encode(payload).await
                     }}
                }}

                #[{async_trait}::async_trait]
                #[automatically_derived]
                impl<'a> {wrpc_transport}::Receive<'a> for {name} {{
                     async fn receive<T>(
                        payload: impl {bytes}::Buf + Send + 'a,
                        rx: &mut (impl {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + Unpin),
                        sub: Option<{wrpc_transport}::AsyncSubscription<T>>,
                     ) -> {anyhow}::Result<(Self, Box<dyn {bytes}::buf::Buf + Send + 'a>)>
                     where
                         T: {futures}::Stream<Item={anyhow}::Result<{bytes}::Bytes>> + Send + Sync + 'static
                     {{
                        use {anyhow}::Context as _;
                        let span = {tracing}::trace_span!("receive");
                        let _enter = span.enter();

                        let (bits, payload) = <Self as {bitflags}::Flags>::Bits::receive(payload, rx, sub).await.context("failed to receive bits")?;
                        let ret = Self::from_bits(bits).context("failed to convert bits into flags")?;
                        Ok((ret, payload))
                     }}
                }}"#,
            anyhow = self.gen.anyhow_path(),
            async_trait = self.gen.async_trait_path(),
            bitflags = self.gen.bitflags_path(),
            bytes = self.gen.bytes_path(),
            futures = self.gen.futures_path(),
            tracing = self.gen.tracing_path(),
            wrpc_transport = self.gen.wrpc_transport_path(),
        );
    }

    fn type_variant(&mut self, id: TypeId, _name: &str, variant: &Variant, docs: &Docs) {
        self.print_typedef_variant(id, variant, docs);
    }

    fn type_option(&mut self, id: TypeId, _name: &str, payload: &Type, docs: &Docs) {
        self.print_typedef_option(id, payload, docs);
    }

    fn type_result(&mut self, id: TypeId, _name: &str, result: &Result_, docs: &Docs) {
        self.print_typedef_result(id, result, docs);
    }

    fn type_enum(&mut self, id: TypeId, name: &str, enum_: &Enum, docs: &Docs) {
        self.print_typedef_enum(id, name, enum_, docs, &[], Box::new(|_| String::new()));

        let name = to_upper_camel_case(name);

        self.print_enum_encode(&name, &enum_.cases);
        self.print_enum_encode(&format!("&{name}"), &enum_.cases);

        self.print_noop_subscribe(&name);
        self.print_noop_subscribe(&format!("&{name}"));

        self.print_enum_receive(&name, &enum_.cases);
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        self.print_typedef_alias(id, ty, docs);
    }

    fn type_list(&mut self, id: TypeId, _name: &str, ty: &Type, docs: &Docs) {
        for (name, _) in self.modes_of(id) {
            self.rustdoc(docs);
            self.push_str(&format!("pub type {name}"));
            self.push_str(" = ");
            self.print_list(ty, TypeMode::owned());
            self.push_str(";\n");
        }
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.rustdoc(docs);
        self.src
            .push_str(&format!("pub type {}", name.to_upper_camel_case()));
        self.src.push_str(" = ");
        self.print_ty(ty, TypeMode::owned());
        self.src.push_str(";\n");
    }
}
