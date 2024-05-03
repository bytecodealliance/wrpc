use crate::{
    int_repr, to_go_ident, to_package_ident, to_upper_camel_case, Deps, FnSig, GoWrpc, Identifier,
    InterfaceName, RustFlagsRepr,
};
use heck::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::mem;
use wit_bindgen_core::{dealias, uwrite, uwriteln, wit_parser::*, Source, TypeInfo};

pub struct InterfaceGenerator<'a> {
    pub src: Source,
    pub(super) identifier: Identifier<'a>,
    pub in_import: bool,
    pub(super) gen: &'a mut GoWrpc,
    pub resolve: &'a Resolve,
    pub deps: Deps,
}

impl InterfaceGenerator<'_> {
    fn print_encode_ty(&mut self, ty: &Type, name: &str, writer: &str) {
        match ty {
            Type::Id(t) => self.print_encode_tyid(*t, name, writer),
            Type::Bool => uwrite!(
                self.src,
                r#"func(v bool, w {wrpc}.ByteWriter) error {{
                if !v {{
                    {slog}.Debug("writing `false` byte")
                    return w.WriteByte(0)
                }}
                {slog}.Debug("writing `true` byte")
                return w.WriteByte(1)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U8 => uwrite!(
                self.src,
                r#"func(v uint8, w {wrpc}.ByteWriter) error {{
                {slog}.Debug("writing u8 byte")
                return w.WriteByte(v)
            }}({name}, {writer})"#,
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U16 => uwrite!(
                self.src,
                r#"func(v uint16, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen16)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u16")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U32 => uwrite!(
                self.src,
                r#"func(v uint32, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u32")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::U64 => uwrite!(
                self.src,
                r#"func(v uint64, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen64)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing u64")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::S8 => uwrite!(
                self.src,
                r#"{errors}.New("writing s8 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S16 => uwrite!(
                self.src,
                r#"{errors}.New("writing s16 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S32 => uwrite!(
                self.src,
                r#"{errors}.New("writing s32 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::S64 => uwrite!(
                self.src,
                r#"{errors}.New("writing s64 not supported yet")"#,
                errors = self.deps.errors(),
            ),
            Type::F32 => uwrite!(
                self.src,
                r#"func(v float32, w {wrpc}.ByteWriter) error {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint32(b, {math}.Float32bits(x))
                {slog}.Debug("writing f32")
	            _, err := w.Write(b)
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::F64 => uwrite!(
                self.src,
                r#"func(v float64, w {wrpc}.ByteWriter) error {{
                b := make([]byte, 4)
                {binary}.LittleEndian.PutUint64(b, {math}.Float64bits(x))
                {slog}.Debug("writing f64")
	            _, err := w.Write(b)
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                math = self.deps.math(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::Char => uwrite!(
                self.src,
                r#"func(v rune, w {wrpc}.ByteWriter) error {{
	            b := make([]byte, {binary}.MaxVarintLen32)
	            i := {binary}.PutUvarint(b, uint64(v))
                {slog}.Debug("writing char")
	            _, err := w.Write(b[:i])
	            return err
            }}({name}, {writer})"#,
                binary = self.deps.binary(),
                slog = self.deps.slog(),
                wrpc = self.deps.wrpc(),
            ),
            Type::String => {
                let fmt = self.deps.fmt();
                let math = self.deps.math();
                let slog = self.deps.slog();
                let wrpc = self.deps.wrpc();

                uwrite!(
                    self.src,
                    r#"func(v string, w {wrpc}.ByteWriter) error {{
	            n := len(v)
	            if n > {math}.MaxUint32 {{
	            	return {fmt}.Errorf("string byte length of %d overflows a 32-bit integer", n)
	            }}
	            {slog}.Debug("writing string byte length", "len", n)
	            if err := "#
                );
                self.print_encode_ty(&Type::U32, "uint32(n)", "w");
                uwrite!(
                    self.src,
                    r#"; err != nil {{
                	return {fmt}.Errorf("failed to write string length of %d: %w", n, err)
                }}
                {slog}.Debug("writing string bytes")
                _, err := w.Write([]byte(v))
                if err != nil {{
                	return {fmt}.Errorf("failed to write string bytes: %w", err)
                }}
                return nil
                }}({name}, {writer})"#
                );
            }
        }
    }

    fn print_encode_tyid(&mut self, ty: TypeId, name: &str, writer: &str) {
        //let ty = &self.resolve.types[id];
        //if ty.name.is_some() {
        //    let name = self.type_path(id);
        //    self.push_str(&name);
        //    return;
        //}

        //match &ty.kind {
        //    TypeDefKind::List(t) => self.print_list(t),

        //    TypeDefKind::Option(t) => {
        //        self.push_str("*");
        //        self.print_ty(t);
        //    }

        //    TypeDefKind::Result(r) => {
        //        self.push_str("wrpc.Result[");
        //        self.print_optional_ty(r.ok.as_ref());
        //        self.push_str(",");
        //        self.print_optional_ty(r.err.as_ref());
        //        self.push_str("]");
        //    }

        //    TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

        //    // Tuple-like records are mapped directly to Rust tuples of
        //    // types. Note the trailing comma after each member to
        //    // appropriately handle 1-tuples.
        //    TypeDefKind::Tuple(t) => {
        //        self.push_str("wrpc.Tuple");
        //        uwrite!(self.src, "{}[", t.types.len());
        //        for ty in t.types.iter() {
        //            self.print_ty(ty);
        //            self.push_str(",");
        //        }
        //        self.push_str("]");
        //    }
        //    TypeDefKind::Resource => {
        //        panic!("unsupported anonymous type reference: resource")
        //    }
        //    TypeDefKind::Record(_) => {
        //        panic!("unsupported anonymous type reference: record")
        //    }
        //    TypeDefKind::Flags(_) => {
        //        panic!("unsupported anonymous type reference: flags")
        //    }
        //    TypeDefKind::Enum(_) => {
        //        panic!("unsupported anonymous type reference: enum")
        //    }
        //    TypeDefKind::Future(ty) => {
        //        self.push_str("wrpc.Receiver[");
        //        self.print_optional_ty(ty.as_ref());
        //        self.push_str("]");
        //    }
        //    TypeDefKind::Stream(stream) => {
        //        self.push_str("wrpc.Receiver[[]");
        //        self.print_optional_ty(stream.element.as_ref());
        //        self.push_str("]");
        //    }

        //    TypeDefKind::Handle(Handle::Own(ty)) => {
        //        self.print_ty(&Type::Id(*ty));
        //    }

        //    TypeDefKind::Handle(Handle::Borrow(ty)) => {
        //        if self.is_exported_resource(*ty) {
        //            let camel = self.resolve.types[*ty]
        //                .name
        //                .as_deref()
        //                .unwrap()
        //                .to_upper_camel_case();
        //            let name = self.type_path_with_name(*ty, format!("{camel}Borrow"));
        //            self.push_str(&name);
        //        } else {
        //            let ty = &Type::Id(*ty);
        //            self.print_ty(ty);
        //        }
        //    }

        //    TypeDefKind::Type(t) => self.print_ty(t),

        //    TypeDefKind::Unknown => unreachable!(),
        //}
    }

    pub(super) fn generate_exports<'a>(
        &mut self,
        identifier: Identifier<'a>,
        funcs: impl Iterator<Item = &'a Function> + Clone + ExactSizeIterator,
    ) -> bool {
        let mut traits = BTreeMap::new();
        let mut funcs_to_export = Vec::new();
        let mut resources_to_drop = Vec::new();

        traits.insert(None, ("Handler".to_string(), Vec::new()));

        if let Identifier::Interface(id, ..) = identifier {
            for (name, id) in self.resolve.interfaces[id].types.iter() {
                match self.resolve.types[*id].kind {
                    TypeDefKind::Resource => {}
                    _ => continue,
                }
                resources_to_drop.push(name);
                let camel = name.to_upper_camel_case();
                traits.insert(Some(*id), (format!("Handler{camel}"), Vec::new()));
            }
        }

        let n = funcs.len();
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
            let sig = FnSig {
                use_item_name: true,
                ..Default::default()
            };
            self.print_docs_and_params(func, &sig);
            if let FunctionKind::Constructor(_) = &func.kind {
                uwriteln!(self.src, " ({}, error)", "Self"); // TODO: Use the correct Go name
            } else {
                self.print_results(&func.results)
            }
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
            )
        }

        for (_, (trait_name, methods)) in traits.iter() {
            uwriteln!(self.src, "type {trait_name} interface {{");
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
            &mut self.src,
            "func ServeInterface(c {wrpc}.Client, h Handler) (stop func() error, err error) {{",
            wrpc = self.deps.wrpc(),
        );
        uwriteln!(self.src, r#"stops := make([]func() error, 0, {n})"#);
        self.src.push_str(
            r#"stop = func() error {
            for _, stop := range stops {
                if err := stop(); err != nil {
                    return err
                }
            }
            return nil
        }
"#,
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
        for (
            i,
            Function {
                kind,
                name,
                params,
                results,
                ..
            },
        ) in funcs_to_export.iter().enumerate()
        {
            if !matches!(kind, FunctionKind::Freestanding) {
                continue;
            }
            if !params.is_empty() {
                eprintln!("skip function with non-empty parameters");
                continue;
            }
            let bytes = self.deps.bytes();
            let context = self.deps.context();
            let fmt = self.deps.fmt();
            let slog = self.deps.slog();
            let wrpc = self.deps.wrpc();
            uwriteln!(
                self.src,
                r#"stop{i}, err := c.Serve("{instance}", "{name}", func(ctx {context}.Context, buffer []byte, tx {wrpc}.Transmitter, inv {wrpc}.IncomingInvocation) error {{"#,
            );
            uwriteln!(
                self.src,
                r#"{slog}.DebugContext(ctx, "accepting handshake")"#,
            );
            self.src
                .push_str("if err := inv.Accept(ctx, nil); err != nil {\n");
            uwriteln!(
                self.src,
                r#"return {fmt}.Errorf("failed to complete handshake: %w", err)"#,
            );
            self.src.push_str("}\n");
            uwriteln!(
                self.src,
                r#"{slog}.DebugContext(ctx, "calling `{instance}.{name}` handler")"#,
            );
            for (i, _) in results.iter_types().enumerate() {
                uwrite!(self.src, "r{i}, ");
            }
            self.push_str("err := h.");
            self.push_str(&name.to_upper_camel_case());
            self.push_str("(ctx");
            for (i, _) in params.iter().enumerate() {
                uwrite!(self.src, ", p{i}");
            }
            self.push_str(")\n");
            self.push_str("if err != nil {\n");
            uwriteln!(
                self.src,
                r#"return {fmt}.Errorf("failed to handle `{instance}.{name}` invocation: %w", err)"#,
            );
            self.push_str("}\n");

            uwriteln!(self.src, r#"var buf {bytes}.Buffer"#,);
            for (i, ty) in results.iter_types().enumerate() {
                self.push_str("if err :=");
                self.print_encode_ty(ty, &format!("r{i}"), "&buf");
                self.push_str("; err != nil {\n");
                uwriteln!(
                    self.src,
                    r#"return {fmt}.Errorf("failed to write result value {i}: %w", err)"#,
                );
                self.src.push_str("}\n");
            }

            uwriteln!(
                self.src,
                r#"{slog}.DebugContext(ctx, "transmitting `{instance}.{name}` result")"#,
            );
            uwriteln!(
                self.src,
                r#"if err := tx.Transmit({context}.Background(), buf.Bytes()); err != nil {{
                    return {fmt}.Errorf("failed to transmit result: %w", err)
                }}"#,
            );
            self.push_str("return nil\n");

            uwriteln!(
                self.src,
                r#"}})
                if err != nil {{
                    return nil, {fmt}.Errorf("failed to serve `{instance}.{name}`: %w", err)
                }}
                stops = append(stops, stop{i})"#,
                fmt = self.deps.fmt(),
            );
        }
        self.push_str("return stop, nil\n");
        self.push_str("}\n");
        true
    }

    fn generate_interface_trait<'a>(
        &mut self,
        trait_name: &str,
        methods: &[Source],
        resource_traits: impl Iterator<Item = (TypeId, &'a str)>,
    ) {
        uwriteln!(self.src, "type {trait_name} interface {{");
        for (id, trait_name) in resource_traits {
            let name = self.resolve.types[id]
                .name
                .as_ref()
                .unwrap()
                .to_upper_camel_case();
            uwriteln!(self.src, "//type {name} {trait_name}");
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
            &mut self.gen.import_modules
        } else {
            &mut self.gen.export_modules
        };
        map.push((module, module_path))
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
        let params = self.print_signature(func, &sig);
        self.src.push_str("{\n");
        self.src.push_str("tx__, txErr__ := wrpc__.Invoke(");
        match func.kind {
            FunctionKind::Freestanding
            | FunctionKind::Static(..)
            | FunctionKind::Constructor(..) => {
                uwrite!(self.src, r#""{instance}""#);
            }
            FunctionKind::Method(..) => {
                self.src.push_str("self.0");
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
        if err != nil {{
            err__ = fmt.Sprintf("failed to invoke `{}`: %w", txErr__)
            return 
        }}
        //wrpc__.1.await.context("failed to transmit parameters")?;
        //Ok(tx__)
        panic("not supported yet")
        return
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

    fn godoc(&mut self, docs: &Docs) {
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

    fn print_signature(&mut self, func: &Function, sig: &FnSig) -> Vec<String> {
        let params = self.print_docs_and_params(func, sig);
        if let FunctionKind::Constructor(_) = &func.kind {
            uwrite!(self.src, " (Self, error)",);
        } else {
            self.print_results(&func.results);
        }
        params
    }

    fn print_docs_and_params(&mut self, func: &Function, sig: &FnSig) -> Vec<String> {
        self.godoc(&func.docs);
        self.rustdoc_params(&func.params, "Parameters");
        // TODO: re-add this when docs are back
        // self.rustdoc_params(&func.results, "Return");

        if self.in_import {
            self.push_str("func ");
        }
        let func_name = if sig.use_item_name {
            if let FunctionKind::Constructor(_) = &func.kind {
                "New"
            } else {
                func.item_name()
            }
        } else {
            &func.name
        };
        if let Some(arg) = &sig.self_arg {
            self.push_str("(");
            self.push_str(arg);
            self.push_str(")");
        }
        self.push_str(&func_name.to_upper_camel_case());
        uwrite!(&mut self.src, "(ctx__ {}.Context, ", self.deps.context());
        if self.in_import {
            uwrite!(&mut self.src, "(wrpc__ {}.Client, ", self.deps.wrpc());
        }
        let mut params = Vec::new();
        for (i, (name, param)) in func.params.iter().enumerate() {
            if let FunctionKind::Method(..) = &func.kind {
                if i == 0 && sig.self_is_first_param {
                    params.push("self".to_string());
                    continue;
                }
            }
            let name = to_go_ident(name);
            self.push_str(&name);
            self.push_str(" ");
            self.print_ty(param);
            self.push_str(",");

            params.push(name);
        }
        self.push_str(")");
        params
    }

    fn print_results(&mut self, results: &Results) {
        self.src.push_str(" (");
        for (i, ty) in results.iter_types().enumerate() {
            uwrite!(self.src, "r{i}__ ");
            self.print_ty(ty);
            self.src.push_str(", ")
        }
        self.push_str("err__ error)\n");
    }

    fn print_ty(&mut self, ty: &Type) {
        match ty {
            Type::Id(t) => self.print_tyid(*t),
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
        }
    }

    fn print_optional_ty(&mut self, ty: Option<&Type>) {
        match ty {
            Some(ty) => self.print_ty(ty),
            None => self.push_str("wrpc.Void"),
        }
    }

    pub fn type_path(&mut self, id: TypeId) -> String {
        self.type_path_with_name(
            id,
            //if owned {
            self.result_name(id), //} else {
                                  //self.param_name(id)
                                  //},
        )
    }

    fn type_path_with_name(&mut self, id: TypeId, name: String) -> String {
        if let TypeOwner::Interface(id) = self.resolve.types[id].owner {
            if let Some(path) = self.path_to_interface(id) {
                return format!("{path}.{name}");
            }
        }
        name
    }

    fn print_tyid(&mut self, id: TypeId) {
        let ty = &self.resolve.types[id];
        if ty.name.is_some() {
            let name = self.type_path(id);
            self.push_str(&name);
            return;
        }

        match &ty.kind {
            TypeDefKind::List(t) => self.print_list(t),

            TypeDefKind::Option(t) => {
                self.push_str("*");
                self.print_ty(t);
            }

            TypeDefKind::Result(r) => {
                self.push_str("wrpc.Result[");
                self.print_optional_ty(r.ok.as_ref());
                self.push_str(",");
                self.print_optional_ty(r.err.as_ref());
                self.push_str("]");
            }

            TypeDefKind::Variant(_) => panic!("unsupported anonymous variant"),

            // Tuple-like records are mapped directly to Rust tuples of
            // types. Note the trailing comma after each member to
            // appropriately handle 1-tuples.
            TypeDefKind::Tuple(t) => {
                self.push_str("wrpc.Tuple");
                uwrite!(self.src, "{}[", t.types.len());
                for ty in t.types.iter() {
                    self.print_ty(ty);
                    self.push_str(",");
                }
                self.push_str("]");
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
                self.push_str("wrpc.Receiver[");
                self.print_optional_ty(ty.as_ref());
                self.push_str("]");
            }
            TypeDefKind::Stream(stream) => {
                self.push_str("wrpc.Receiver[[]");
                self.print_optional_ty(stream.element.as_ref());
                self.push_str("]");
            }

            TypeDefKind::Handle(Handle::Own(ty)) => {
                self.print_ty(&Type::Id(*ty));
            }

            TypeDefKind::Handle(Handle::Borrow(ty)) => {
                if self.is_exported_resource(*ty) {
                    let camel = self.resolve.types[*ty]
                        .name
                        .as_deref()
                        .unwrap()
                        .to_upper_camel_case();
                    let name = self.type_path_with_name(*ty, format!("{camel}Borrow"));
                    self.push_str(&name);
                } else {
                    let ty = &Type::Id(*ty);
                    self.print_ty(ty);
                }
            }

            TypeDefKind::Type(t) => self.print_ty(t),

            TypeDefKind::Unknown => unreachable!(),
        }
    }

    fn print_noop_subscribe(&mut self, name: &str) {
        uwrite!(self.src, "impl wrpc::Subscribe for {name} {{}}",);
    }

    fn print_list(&mut self, ty: &Type) {
        self.push_str("[]");
        self.print_ty(ty);
    }

    fn print_struct_encode(&mut self, name: &str, ty: &Record) {
        self.push_str("impl");
        uwrite!(self.src, " wrpc::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{");
        if !ty.fields.is_empty() {
            uwriteln!(self.src, r#"let span = slog::trace_span!("encode");"#);
            self.push_str("let _enter = span.enter();\n");
            self.push_str("let ");
            self.push_str(name.trim_start_matches('&'));
            self.push_str("{\n");
            for Field { name, .. } in ty.fields.iter() {
                let name_rust = to_go_ident(name);
                uwriteln!(self.src, "{name_rust} f_{name_rust},")
            }
            self.push_str("} = self;\n");
            for Field { name, .. } in ty.fields.iter() {
                let name_rust = to_go_ident(name);
                uwriteln!(
                    self.src,
                    r#"let f_{name_rust} = f_{name_rust}.encode(&mut payload).await.context("failed to encode `{name}`")?;"#,
                )
            }
            self.push_str("if false");
            for Field { name, .. } in ty.fields.iter() {
                uwrite!(self.src, r#" || f_{}.is_some()"#, to_go_ident(name))
            }
            self.push_str("{\n");
            uwrite!(self.src, "return Ok(Some(wrpc::AsyncValue::Record(vec![");
            for Field { name, .. } in ty.fields.iter() {
                uwrite!(self.src, r#"f_{},"#, to_go_ident(name))
            }
            self.push_str("\n])))}\n");
        }
        self.push_str("Ok(None)\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_struct_subscribe(&mut self, name: &str, ty: &Record) {
        self.push_str("impl");
        uwrite!(self.src, " wrpc::Subscribe for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "async fn subscribe<T: wrpc::Subscriber + Send + Sync>(subscriber: &T, subject: T::Subject) -> Result<Option<wrpc::AsyncSubscription<T::Stream>>, T::SubscribeError> {{");
        if !ty.fields.is_empty() {
            uwriteln!(self.src, r#"let span = slog::trace_span!("subscribe");"#);
            self.push_str("let _enter = span.enter();\n");
            for (i, Field { name, ty, .. }) in ty.fields.iter().enumerate() {
                let name_rust = to_go_ident(name);
                uwrite!(self.src, r#"let f_{name_rust} = <"#,);
                self.print_ty(ty);
                uwrite!(
                    self.src,
                    r#">::subscribe(subscriber, subject.child(Some({i}))).await?;"#,
                )
            }
            self.push_str("if false");
            for Field { name, .. } in ty.fields.iter() {
                uwrite!(self.src, r#" || f_{}.is_some()"#, to_go_ident(name))
            }
            self.push_str("{\n");
            uwrite!(
                self.src,
                "return Ok(Some(wrpc::AsyncSubscription::Record(vec!["
            );
            for Field { name, .. } in ty.fields.iter() {
                uwrite!(self.src, r#"f_{},"#, to_go_ident(name))
            }
            self.push_str("\n])))}\n");
        }
        self.push_str("Ok(None)\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_struct_receive(&mut self, name: &str, ty: &Record) {
        uwriteln!(self.src, "");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " wrpc::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        self.push_str(" {");
        uwriteln!(
            self.src,
            r#"
    #[allow(unused_mut)]
    async fn receive<T>(
        payload: impl bytes::Buf + Send + 'wrpc_receive_,
        mut rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
        sub: Option<wrpc::AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf + Send + 'wrpc_receive_>)>
        where
            T: futures::Stream<Item=anyhow::Result<[]byte>> {{"#
        );
        if !ty.fields.is_empty() {
            uwriteln!(self.src, r#"let span = slog::trace_span!("receive");"#);
            self.push_str("let _enter = span.enter();\n");
            uwriteln!(
                self.src,
                "let mut sub = sub.map(wrpc::AsyncSubscription::try_unwrap_record).transpose()?;"
            );
            for (i, Field { name, .. }) in ty.fields.iter().enumerate() {
                let name_rust = to_go_ident(name);
                uwriteln!(
                    self.src,
                    r#"let (f_{name_rust}, payload) = wrpc::Receive::receive(
                        payload,
                        &mut rx,
                        sub.as_mut().and_then(|sub| sub.get_mut({i})).and_then(Option::take)
                    )
                    .await
                    .context("failed to receive `{name}`")?;"#,
                )
            }
        }
        self.push_str("Ok((Self {\n");
        for Field { name, .. } in ty.fields.iter() {
            let name_rust = to_go_ident(name);
            uwrite!(self.src, r#"{name_rust}: f_{name_rust},"#)
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
        uwriteln!(self.src, "");
        self.push_str("impl");
        uwrite!(self.src, " wrpc::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "#[allow(unused_mut)]");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{");
        uwriteln!(self.src, r#"let span = slog::trace_span!("encode");"#);
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
            uwriteln!(self.src, "wrpc::encode_discriminant(&mut payload, {i})?;");
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
        self.push_str("impl");
        uwrite!(self.src, " wrpc::Subscribe for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "async fn subscribe<T: wrpc::Subscriber + Send + Sync>(subscriber: &T, subject: T::Subject) -> Result<Option<wrpc::AsyncSubscription<T::Stream>>, T::SubscribeError> {{");
        uwriteln!(self.src, r#"let span = slog::trace_span!("subscribe");"#);
        self.push_str("let _enter = span.enter();\n");
        for (i, (_, _, payload)) in cases.clone().into_iter().enumerate() {
            if let Some(ty) = payload {
                uwrite!(self.src, r#"let c_{i} = <"#,);
                self.print_ty(ty);
                uwrite!(
                    self.src,
                    r#">::subscribe(subscriber, subject.child(Some({i}))).await?;"#,
                )
            }
        }
        self.push_str("if false");
        for (i, (_, _, payload)) in cases.clone().into_iter().enumerate() {
            if payload.is_some() {
                uwrite!(self.src, r#" || c_{i}.is_some()"#)
            }
        }
        self.push_str("{\n");
        uwrite!(
            self.src,
            "return Ok(Some(wrpc::AsyncSubscription::Variant(vec!["
        );
        for (i, (_, _, payload)) in cases.into_iter().enumerate() {
            if payload.is_some() {
                uwrite!(self.src, r#"c_{i},"#)
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
        uwriteln!(self.src, "");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " wrpc::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        uwriteln!(
            self.src,
            r#" {{
        async fn receive<T>(
            payload: impl bytes::Buf + Send + 'wrpc_receive_,
            mut rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
            sub: Option<wrpc::AsyncSubscription<T>>,
        ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf + Send + 'wrpc_receive_>)>
            where
                T: futures::Stream<Item=anyhow::Result<[]byte>>
            {{
                let span = slog::trace_span!("receive");
                let _enter = span.enter();
                let (disc, payload) = wrpc::receive_discriminant(
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
                                anyhow::Ok(sub.get_mut({i}).and_then(Option::take))
                            }})
                            .transpose()?
                            .flatten();
                        let (nested, payload) = wrpc::Receive::receive(payload, rx, sub)
                            .await
                            .context("failed to receive nested variant value")?;
                        Ok((Self::{case_name}(nested), payload))
                    }}"#
                );
            } else {
                uwriteln!(self.src, "Ok((Self::{case_name}, payload)),")
            }
        }
        uwriteln!(
            self.src,
            r#"_ => anyhow::bail!("unknown variant discriminant `{{disc}}`")"#
        );
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_enum_encode(&mut self, name: &str, cases: &[EnumCase]) {
        uwriteln!(self.src, "");
        self.push_str("impl");
        uwrite!(self.src, " wrpc::Encode for ");
        self.push_str(name);
        self.push_str(" {\n");
        uwriteln!(self.src, "#[allow(unused_mut)]");
        uwriteln!(self.src, "async fn encode(self, mut payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{");
        uwriteln!(self.src, r#"let span = slog::trace_span!("encode");"#);
        self.push_str("let _enter = span.enter();\n");
        self.push_str("match self {\n");
        for (i, case) in cases.iter().enumerate() {
            let case = case.name.to_upper_camel_case();
            self.push_str(name.trim_start_matches('&'));
            self.push_str("::");
            self.push_str(&case);
            self.push_str(" => {\n");
            uwriteln!(self.src, "wrpc::encode_discriminant(&mut payload, {i})?;");
            self.push_str("Ok(None)\n");
            self.push_str("}\n");
        }
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn print_enum_receive(&mut self, name: &str, cases: &[EnumCase]) {
        uwriteln!(self.src, "");
        self.push_str("impl<'wrpc_receive_>");
        uwrite!(self.src, " wrpc::Receive<'wrpc_receive_> for ");
        self.push_str(name);
        uwriteln!(
            self.src,
            r#" {{
        async fn receive<T>(
            payload: impl bytes::Buf + Send + 'wrpc_receive_,
            mut rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
            sub: Option<wrpc::AsyncSubscription<T>>,
        ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf + Send + 'wrpc_receive_>)>
            where
                T: futures::Stream<Item=anyhow::Result<[]byte>>
            {{
                let span = slog::trace_span!("receive");
                let _enter = span.enter();
                let (disc, payload) = wrpc::receive_discriminant(
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
            r#"_ => anyhow::bail!("unknown enum discriminant `{{disc}}`")"#
        );
        self.push_str("}\n");
        self.push_str("}\n");
        self.push_str("}\n");
    }

    fn int_repr(&mut self, repr: Int) {
        self.push_str(int_repr(repr));
    }

    fn modes_of(&self, ty: TypeId) -> Vec<String> {
        let info = self.info(ty);
        // If this type isn't actually used, no need to generate it.
        if !info.owned && !info.borrowed {
            return vec![];
        }
        vec![self.result_name(ty)]
    }

    fn print_typedef_record(&mut self, id: TypeId, record: &Record, docs: &Docs) {
        let info = self.info(id);
        // We use a BTree set to make sure we don't have any duplicates and we have a stable order
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("pub struct {}", name));
            self.push_str(" {\n");
            for Field { name, ty, docs } in record.fields.iter() {
                self.godoc(docs);
                self.push_str(&to_go_ident(name));
                self.push_str(": ");
                self.print_ty(ty);
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
            self.push_str(&format!("f.debug_struct(\"{}\")", name));
            for field in record.fields.iter() {
                self.push_str(&format!(
                    ".field(\"{}\", &self.{})",
                    field.name,
                    to_go_ident(&field.name)
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
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("pub enum {name}"));
            self.push_str(" {\n");
            for (case_name, docs, payload) in cases.clone() {
                self.godoc(docs);
                self.push_str(&case_name);
                if let Some(ty) = payload {
                    self.push_str("(");
                    self.print_ty(ty);
                    self.push_str(")")
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
            self.push_str(&format!("f.debug_tuple(\"{}::{}\")", name, case_name));
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
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {}", name));
            self.push_str("= Option<");
            self.print_ty(payload);
            self.push_str(">;\n");
        }
    }

    fn print_typedef_result(&mut self, id: TypeId, result: &Result_, docs: &Docs) {
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {}", name));
            self.push_str("= Result<");
            self.print_optional_ty(result.ok.as_ref());
            self.push_str(",");
            self.print_optional_ty(result.err.as_ref());
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
        self.godoc(docs);
        for attr in attrs {
            self.push_str(&format!("{}\n", attr));
        }
        self.push_str("#[repr(");
        self.int_repr(enum_.tag());
        self.push_str(")]\n");
        // We use a BTree set to make sure we don't have any duplicates and a stable order
        self.push_str(&format!("pub enum {name} {{\n"));
        for case in enum_.cases.iter() {
            self.godoc(&case.docs);
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
            for case in enum_.cases.iter() {
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
            for case in enum_.cases.iter() {
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
            )
        }
    }

    fn print_typedef_alias(&mut self, id: TypeId, ty: &Type, docs: &Docs) {
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {name}"));
            self.push_str(" = ");
            self.print_ty(ty);
            self.push_str(";\n");
        }

        if self.is_exported_resource(id) {
            self.godoc(docs);
            let name = self.resolve.types[id].name.as_ref().unwrap();
            let name = name.to_upper_camel_case();
            self.push_str(&format!("type {name}Borrow"));
            self.push_str(" = ");
            self.print_ty(ty);
            self.push_str("Borrow");
            self.push_str(";\n");
        }
    }

    fn param_name(&self, ty: TypeId) -> String {
        let info = self.info(ty);
        let name = to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap());
        if self.uses_two_names(&info) {
            format!("{}Param", name)
        } else {
            name
        }
    }

    fn result_name(&self, ty: TypeId) -> String {
        let info = self.info(ty);
        let name = to_upper_camel_case(self.resolve.types[ty].name.as_ref().unwrap());
        if self.uses_two_names(&info) {
            format!("{}Result", name)
        } else {
            name
        }
    }

    fn uses_two_names(&self, info: &TypeInfo) -> bool {
        info.borrowed
            && info.owned
            // ... and they have a list ...
            && info.has_list
            // ... and if there's NOT an `own` handle since those are always
            // done by ownership.
            && !info.has_own_handle
    }

    fn path_to_interface(&mut self, interface: InterfaceId) -> Option<String> {
        let InterfaceName {
            import_name,
            import_path,
        } = &self.gen.interface_names[&interface];
        if let Identifier::Interface(cur, _) = self.identifier {
            if cur == interface {
                return None;
            }
        }
        Some(self.deps.import(import_name.clone(), import_path.clone()))
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
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(&mut self, id: TypeId, _name: &str, record: &Record, docs: &Docs) {
        self.print_typedef_record(id, record, docs);
    }

    fn type_resource(&mut self, _id: TypeId, name: &str, docs: &Docs) {
        self.godoc(docs);
        let camel = to_upper_camel_case(name);

        if self.in_import {
            uwriteln!(
                self.src,
                r#"
#[derive(Debug)]
pub struct {camel}(String);


impl wrpc::Encode for {camel} {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        self.0.encode(payload).await
     }}
}}


impl wrpc::Encode for &{camel} {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        self.0.as_str().encode(payload).await
     }}
}}

impl wrpc::Subscribe for {camel} {{}}

impl wrpc::Subscribe for &{camel} {{}}


impl<'a> wrpc::Receive<'a> for {camel} {{
     async fn receive<T>(
        payload: impl bytes::Buf,
        rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
        sub: Option<wrpc::AsyncSubscription<T>>,
     ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf>)>
     where
         T: futures::Stream<Item=anyhow::Result<[]byte>>
     {{
        let span = slog::trace_span!("receive");
        let _enter = span.enter();
        let (subject, payload) = wrpc::Receive::receive(payload, rx, sub).await?;
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


impl wrpc::Encode for {camel}Borrow {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}


impl wrpc::Encode for &{camel}Borrow {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}

impl wrpc::Subscribe for {camel}Borrow {{}}

impl wrpc::Subscribe for &{camel}Borrow {{}}


impl<'a> wrpc::Receive<'a> for {camel}Borrow {{
     async fn receive<T>(
        payload: impl bytes::Buf,
        rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
        sub: Option<wrpc::AsyncSubscription<T>>,
     ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf>)>
     where
         T: futures::Stream<Item=anyhow::Result<[]byte>>
     {{
        let span = slog::trace_span!("receive");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}


impl wrpc::Encode for {camel} {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}


impl wrpc::Encode for &{camel} {{
     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
        let span = slog::trace_span!("encode");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}

impl wrpc::Subscribe for {camel} {{}}

impl wrpc::Subscribe for &{camel} {{}}


impl<'a> wrpc::Receive<'a> for {camel} {{
     async fn receive<T>(
        payload: impl bytes::Buf,
        rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
        sub: Option<wrpc::AsyncSubscription<T>>,
     ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf>)>
     where
         T: futures::Stream<Item=anyhow::Result<[]byte>>
     {{
        let span = slog::trace_span!("receive");
        let _enter = span.enter();
        anyhow::bail!("exported resources not supported yet")
     }}
}}
"#,
            );
        }
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, tuple: &Tuple, docs: &Docs) {
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {}", name));
            self.push_str(" = (");
            for ty in tuple.types.iter() {
                self.print_ty(ty);
                self.push_str(",");
            }
            self.push_str(");\n");
        }
    }

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.src.push_str(&format!("bitflags::bitflags! {{\n",));
        self.godoc(docs);
        let repr = RustFlagsRepr::new(flags);
        let name = to_upper_camel_case(name);
        self.src.push_str(&format!(
            "#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]\npub struct {name}: {repr} {{\n",
        ));
        for (i, flag) in flags.flags.iter().enumerate() {
            self.godoc(&flag.docs);
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
                impl wrpc::Encode for {name} {{
                     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
                        let span = slog::trace_span!("encode");
                        let _enter = span.enter();
                        self.bits().encode(payload).await
                     }}
                }}

                impl wrpc::Encode for &{name} {{
                     async fn encode(self, payload: &mut (impl bytes::BufMut + Send)) -> anyhow::Result<Option<wrpc::AsyncValue>> {{
                        let span = slog::trace_span!("encode");
                        let _enter = span.enter();
                        self.bits().encode(payload).await
                     }}
                }}

                impl<'a> wrpc::Receive<'a> for {name} {{
                     async fn receive<T>(
                        payload: impl bytes::Buf,
                        rx: &mut (impl futures::Stream<Item=anyhow::Result<[]byte>> + Send + Sync + Unpin),
                        sub: Option<wrpc::AsyncSubscription<T>>,
                     ) -> anyhow::Result<(Self, Box<dyn bytes::buf::Buf>)>
                     where
                         T: futures::Stream<Item=anyhow::Result<[]byte>>
                     {{
                        let span = slog::trace_span!("receive");
                        let _enter = span.enter();

                        let (bits, payload) = <Self as bitflags::Flags>::Bits::receive(payload, rx, sub).await.context("failed to receive bits")?;
                        let ret = Self::from_bits(bits).context("failed to convert bits into flags")?;
                        Ok((ret, payload))
                     }}
                }}"#,
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
        for name in self.modes_of(id) {
            self.godoc(docs);
            self.push_str(&format!("type {}", name));
            self.push_str(" = ");
            self.print_list(ty);
            self.push_str(";\n");
        }
    }

    fn type_builtin(&mut self, _id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        self.godoc(docs);
        self.src
            .push_str(&format!("type {}", name.to_upper_camel_case()));
        self.src.push_str(" = ");
        self.print_ty(ty);
        self.src.push_str(";\n");
    }
}
