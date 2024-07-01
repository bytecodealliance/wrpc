use crate::interface::InterfaceGenerator;
use anyhow::{bail, Result};
use core::str::FromStr;
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::io::{Read, Write};
use std::mem;
use std::process::{Command, Stdio};
use wit_bindgen_core::wit_parser::{
    Flags, FlagsRepr, Function, Int, InterfaceId, PackageId, Resolve, SizeAlign, TypeId, World,
    WorldId, WorldItem, WorldKey,
};
use wit_bindgen_core::{uwriteln, Files, InterfaceGenerator as _, Source, Types, WorldGenerator};

mod interface;

struct InterfaceName {
    /// True when this interface name has been remapped through the use of `with` in the `bindgen!`
    /// macro invocation.
    remapped: bool,

    /// The string name for this interface.
    path: String,
}

#[derive(Default)]
struct RustWrpc {
    types: Types,
    src: Source,
    opts: Opts,
    import_modules: Vec<(String, Vec<String>)>,
    export_modules: Vec<(String, Vec<String>)>,
    skip: HashSet<String>,
    interface_names: HashMap<InterfaceId, InterfaceName>,
    import_funcs_called: bool,
    with_name_counter: usize,
    // Track which interfaces were generated. Remapped interfaces provided via `with`
    // are required to be used.
    generated_interfaces: HashSet<String>,
    world: Option<WorldId>,

    export_paths: Vec<String>,
    with: GenerationConfiguration,
}

#[derive(Default)]
struct GenerationConfiguration {
    map: HashMap<String, InterfaceGeneration>,
    generate_by_default: bool,
}

impl GenerationConfiguration {
    fn get(&self, key: &str) -> Option<&InterfaceGeneration> {
        self.map.get(key).or_else(|| {
            self.generate_by_default
                .then_some(&InterfaceGeneration::Generate)
        })
    }

    fn insert(&mut self, name: String, generate: InterfaceGeneration) {
        self.map.insert(name, generate);
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &InterfaceGeneration)> {
        self.map.iter()
    }
}

/// How an interface should be generated.
enum InterfaceGeneration {
    /// Remapped to some other type
    Remap(String),
    /// Generate the interface
    Generate,
}

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Opts {
    /// Whether or not `rustfmt` is executed to format generated code.
    #[cfg_attr(feature = "clap", arg(long))]
    pub rustfmt: bool,

    /// If true, code generation should qualify any features that depend on
    /// `std` with `cfg(feature = "std")`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub std_feature: bool,

    /// Names of functions to skip generating bindings for.
    #[cfg_attr(feature = "clap", arg(long))]
    pub skip: Vec<String>,

    /// The optional path to the `anyhow` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::anyhow`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub anyhow_path: Option<String>,

    /// The optional path to the bitflags crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::bitflags`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub bitflags_path: Option<String>,

    /// The optional path to the `bytes` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::bytes`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub bytes_path: Option<String>,

    /// The optional path to the `futures` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::futures`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub futures_path: Option<String>,

    /// The optional path to the `tokio` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::tokio`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub tokio_path: Option<String>,

    /// The optional path to the `tokio-util` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::tokio_util`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub tokio_util_path: Option<String>,

    /// The optional path to the `tracing` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::tracing`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub tracing_path: Option<String>,

    /// The optional path to the `wasm-tokio` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::wasm_tokio`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub wasm_tokio_path: Option<String>,

    /// The optional path to the `wrpc-transport` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::wrpc_transport`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub wrpc_transport_path: Option<String>,

    /// Additional derive attributes to add to generated types. If using in a CLI, this flag can be
    /// specified multiple times to add multiple attributes.
    ///
    /// These derive attributes will be added to any generated structs or enums
    #[cfg_attr(feature = "clap", arg(long = "additional_derive_attribute", short = 'd', default_values_t = Vec::<String>::new()))]
    pub additional_derive_attributes: Vec<String>,

    /// Remapping of interface names to rust module names.
    ///
    /// Argument must be of the form `k=v` and this option can be passed
    /// multiple times or one option can be comma separated, for example
    /// `k1=v1,k2=v2`.
    #[cfg_attr(feature = "clap", arg(long, value_parser = clap::value_parser!(WithGeneration), value_delimiter = ','))]
    pub with: WithGeneration,

    /// Add the specified suffix to the name of the custome section containing
    /// the component type.
    #[cfg_attr(feature = "clap", arg(long))]
    pub type_section_suffix: Option<String>,

    /// Whether to generate unused structures, not generated by default (false)
    #[cfg_attr(feature = "clap", arg(long))]
    pub generate_unused_types: bool,
}

impl Opts {
    #[must_use]
    pub fn build(self) -> Box<dyn WorldGenerator> {
        let mut r = RustWrpc::new();
        r.skip = self.skip.iter().cloned().collect();
        r.opts = self;
        Box::new(r)
    }
}

impl RustWrpc {
    fn new() -> RustWrpc {
        RustWrpc::default()
    }

    fn interface<'a>(
        &'a mut self,
        identifier: Identifier<'a>,
        resolve: &'a Resolve,
        in_import: bool,
    ) -> InterfaceGenerator<'a> {
        let mut sizes = SizeAlign::default();
        sizes.fill(resolve);

        InterfaceGenerator {
            identifier,
            src: Source::default(),
            in_import,
            gen: self,
            resolve,
        }
    }

    fn emit_modules(&mut self, modules: Vec<(String, Vec<String>)>) {
        #[derive(Default)]
        struct Module {
            submodules: BTreeMap<String, Module>,
            contents: Vec<String>,
        }
        let mut map = Module::default();
        for (module, path) in modules {
            let mut cur = &mut map;
            for name in &path[..path.len() - 1] {
                cur = cur
                    .submodules
                    .entry(name.clone())
                    .or_insert(Module::default());
            }
            cur.contents.push(module);
        }
        emit(&mut self.src, map);
        fn emit(me: &mut Source, module: Module) {
            for (name, submodule) in module.submodules {
                // Ignore dead-code warnings. If the bindings are only used
                // within a crate, and not exported to a different crate, some
                // parts may be unused, and that's ok.
                uwriteln!(me, "#[allow(dead_code)]");

                uwriteln!(me, "pub mod {name} {{");
                emit(me, submodule);
                uwriteln!(me, "}}");
            }
            for submodule in module.contents {
                uwriteln!(me, "{submodule}");
            }
        }
    }

    fn anyhow_path(&self) -> &str {
        self.opts
            .anyhow_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::anyhow")
    }

    fn bitflags_path(&self) -> &str {
        self.opts
            .bitflags_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::bitflags")
    }

    fn bytes_path(&self) -> &str {
        self.opts
            .bytes_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::bytes")
    }

    fn futures_path(&self) -> &str {
        self.opts
            .futures_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::futures")
    }

    fn tokio_path(&self) -> &str {
        self.opts
            .tokio_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::tokio")
    }

    fn tokio_util_path(&self) -> &str {
        self.opts
            .tokio_util_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::tokio_util")
    }

    fn tracing_path(&self) -> &str {
        self.opts
            .tracing_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::tracing")
    }

    fn wasm_tokio_path(&self) -> &str {
        self.opts
            .wasm_tokio_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::wasm_tokio")
    }

    fn wrpc_transport_path(&self) -> &str {
        self.opts
            .wrpc_transport_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::wrpc_transport")
    }

    fn name_interface(
        &mut self,
        resolve: &Resolve,
        id: InterfaceId,
        name: &WorldKey,
        is_export: bool,
    ) -> Result<bool> {
        let with_name = resolve.name_world_key(name);
        let Some(remapping) = self.with.get(&with_name) else {
            bail!("no remapping found for {with_name:?} - use the `generate!` macro's `with` option to force the interface to be generated or specify where it is already defined:
```
with: {{\n\t{with_name:?}: generate\n}}
```")
        };
        self.generated_interfaces.insert(with_name);
        let entry = match remapping {
            InterfaceGeneration::Remap(remapped_path) => {
                let name = format!("__with_name{}", self.with_name_counter);
                self.with_name_counter += 1;
                uwriteln!(self.src, "use {remapped_path} as {name};");
                InterfaceName {
                    remapped: true,
                    path: name,
                }
            }
            InterfaceGeneration::Generate => {
                let path = compute_module_path(name, resolve, is_export).join("::");

                InterfaceName {
                    remapped: false,
                    path,
                }
            }
        };

        let remapped = entry.remapped;
        self.interface_names.insert(id, entry);

        Ok(remapped)
    }

    /// Generates a `serve` function for the `world_id` specified.
    ///
    /// This will generate a macro which will then itself invoke all the
    /// other macros collected in `self.export_paths` prior. All these macros
    /// are woven together in this single invocation.
    fn finish_serve_function(&mut self) {
        const ROOT: &str = "Handler<T::Context>";
        let mut traits: Vec<String> = self
            .export_paths
            .iter()
            .map(|path| {
                if path.is_empty() {
                    ROOT.to_string()
                } else {
                    format!("{path}::{ROOT}")
                }
            })
            .collect();
        let bound = match traits.len() {
            0 => return,
            1 => traits.pop().unwrap(),
            _ => traits.join(" + "),
        };
        let anyhow = self.anyhow_path().to_string();
        let futures = self.futures_path().to_string();
        let tokio = self.tokio_path().to_string();
        let wrpc_transport = self.wrpc_transport_path().to_string();
        uwriteln!(
            self.src,
            r#"
pub async fn serve<T: {wrpc_transport}::Serve>(
    wrpc: &T,
    handler: impl {bound} + Send + Sync + Clone + 'static,
    shutdown: impl ::core::future::Future<Output = ()>,
) -> {anyhow}::Result<()> {{
    use {futures}::FutureExt as _;
    let shutdown = shutdown.shared();
    {tokio}::try_join!("#
        );
        for path in &self.export_paths {
            if !path.is_empty() {
                self.src.push_str(path);
                self.src.push_str("::");
            }
            self.src
                .push_str("serve_interface(wrpc, handler.clone(), shutdown.clone()),");
        }
        uwriteln!(
            self.src,
            r#"
    )?;
    Ok(())
}}"#
        );
    }
}

/// If the package `id` is the only package with its namespace/name combo
/// then pass through the name unmodified. If, however, there are multiple
/// versions of this package then the package module is going to get version
/// information.
fn name_package_module(resolve: &Resolve, id: PackageId) -> String {
    let pkg = &resolve.packages[id];
    let versions_with_same_name = resolve
        .packages
        .iter()
        .filter_map(|(_, p)| {
            if p.name.namespace == pkg.name.namespace && p.name.name == pkg.name.name {
                Some(&p.name.version)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let base = pkg.name.name.to_snake_case();
    if versions_with_same_name.len() == 1 {
        return base;
    }

    let version = match &pkg.name.version {
        Some(version) => version,
        // If this package didn't have a version then don't mangle its name
        // and other packages with the same name but with versions present
        // will have their names mangled.
        None => return base,
    };

    // Here there's multiple packages with the same name that differ only in
    // version, so the version needs to be mangled into the Rust module name
    // that we're generating. This in theory could look at all of
    // `versions_with_same_name` and produce a minimal diff, e.g. for 0.1.0
    // and 0.2.0 this could generate "foo1" and "foo2", but for now
    // a simpler path is chosen to generate "foo0_1_0" and "foo0_2_0".
    let version = version
        .to_string()
        .replace(['.', '-', '+'], "_")
        .to_snake_case();
    format!("{base}{version}")
}

impl WorldGenerator for RustWrpc {
    fn preprocess(&mut self, resolve: &Resolve, world: WorldId) {
        wit_bindgen_core::generated_preamble(&mut self.src, env!("CARGO_PKG_VERSION"));

        // Render some generator options to assist with debugging and/or to help
        // recreate it if the original generation command is lost.
        uwriteln!(self.src, "// Options used:");
        if !self.opts.skip.is_empty() {
            uwriteln!(self.src, "//   * skip: {:?}", self.opts.skip);
        }
        if !self.opts.additional_derive_attributes.is_empty() {
            uwriteln!(
                self.src,
                "//   * additional derives {:?}",
                self.opts.additional_derive_attributes
            );
        }
        for (k, v) in self.opts.with.iter() {
            uwriteln!(self.src, "//   * with {k:?} = {v:?}");
        }
        self.types.analyze(resolve);
        self.world = Some(world);

        let world = &resolve.worlds[world];
        // Specify that all imports local to the world's package should be generated
        for (key, item) in world.imports.iter().chain(world.exports.iter()) {
            if let WorldItem::Interface { id, .. } = item {
                if resolve.interfaces[*id].package == world.package {
                    let name = resolve.name_world_key(key);
                    if self.with.get(&name).is_none() {
                        self.with.insert(name, InterfaceGeneration::Generate);
                    }
                }
            }
        }
        for (k, v) in self.opts.with.iter() {
            self.with.insert(k.clone(), v.clone().into());
        }
        self.with.generate_by_default = self.opts.with.generate_by_default;
    }

    fn import_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let mut gen = self.interface(Identifier::Interface(id, name), resolve, true);
        let (snake, module_path) = gen.start_append_submodule(name);
        if gen.gen.name_interface(resolve, id, name, false)? {
            return Ok(());
        }
        gen.types(id);

        let interface = &resolve.interfaces[id];
        let name = match name {
            WorldKey::Name(s) => s.to_string(),
            WorldKey::Interface(..) => interface
                .name
                .as_ref()
                .expect("interface name missing")
                .to_string(),
        };
        let instance = if let Some(package) = interface.package {
            resolve.id_of_name(package, &name)
        } else {
            name
        };
        gen.generate_imports(&instance, resolve.interfaces[id].functions.values());

        gen.finish_append_submodule(&snake, module_path);

        Ok(())
    }

    fn import_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) {
        self.import_funcs_called = true;

        let mut gen = self.interface(Identifier::World(world), resolve, true);
        let World {
            ref name, package, ..
        } = resolve.worlds[world];
        let instance = if let Some(package) = package {
            resolve.id_of_name(package, name)
        } else {
            name.to_string()
        };
        gen.generate_imports(&instance, funcs.iter().map(|(_, func)| *func));

        let src = gen.finish();
        self.src.push_str(&src);
    }

    fn export_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let mut gen = self.interface(Identifier::Interface(id, name), resolve, false);
        let (snake, module_path) = gen.start_append_submodule(name);
        if gen.gen.name_interface(resolve, id, name, true)? {
            return Ok(());
        }
        gen.types(id);
        let exports = gen.generate_exports(
            Identifier::Interface(id, name),
            resolve.interfaces[id].functions.values(),
        );
        gen.finish_append_submodule(&snake, module_path);
        if exports {
            self.export_paths
                .push(self.interface_names[&id].path.clone());
        }
        Ok(())
    }

    fn export_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) -> Result<()> {
        let mut gen = self.interface(Identifier::World(world), resolve, false);
        let exports = gen.generate_exports(Identifier::World(world), funcs.iter().map(|f| f.1));
        let src = gen.finish();
        self.src.push_str(&src);
        if exports {
            self.export_paths.push(String::new());
        }
        Ok(())
    }

    fn import_types(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        types: &[(&str, TypeId)],
        _files: &mut Files,
    ) {
        let mut gen = self.interface(Identifier::World(world), resolve, true);
        for (name, ty) in types {
            gen.define_type(name, *ty);
        }
        let src = gen.finish();
        self.src.push_str(&src);
    }

    fn finish_imports(&mut self, resolve: &Resolve, world: WorldId, files: &mut Files) {
        if !self.import_funcs_called {
            // We call `import_funcs` even if the world doesn't import any
            // functions since one of the side effects of that method is to
            // generate `struct`s for any imported resources.
            self.import_funcs(resolve, world, &[], files);
        }
    }

    fn finish(&mut self, resolve: &Resolve, world: WorldId, files: &mut Files) -> Result<()> {
        let name = &resolve.worlds[world].name;

        let imports = mem::take(&mut self.import_modules);
        self.emit_modules(imports);
        let exports = mem::take(&mut self.export_modules);
        self.emit_modules(exports);

        self.finish_serve_function();

        let mut src = mem::take(&mut self.src);
        if self.opts.rustfmt {
            let mut child = Command::new("rustfmt")
                .arg("--edition=2018")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("failed to spawn `rustfmt`");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(src.as_bytes())
                .unwrap();
            src.as_mut_string().truncate(0);
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(src.as_mut_string())
                .unwrap();
            let status = child.wait().unwrap();
            assert!(status.success());
        }

        let module_name = name.to_snake_case();
        files.push(&format!("{module_name}.rs"), src.as_bytes());

        let remapped_keys = self
            .with
            .iter()
            .map(|(k, _)| k)
            .cloned()
            .collect::<HashSet<String>>();

        let mut unused_keys = remapped_keys
            .difference(&self.generated_interfaces)
            .collect::<Vec<&String>>();

        unused_keys.sort();

        if !unused_keys.is_empty() {
            bail!("unused remappings provided via `with`: {unused_keys:?}");
        }

        Ok(())
    }
}

fn compute_module_path(name: &WorldKey, resolve: &Resolve, is_export: bool) -> Vec<String> {
    let mut path = Vec::new();
    if is_export {
        path.push("exports".to_string());
    }
    match name {
        WorldKey::Name(name) => {
            path.push(to_rust_ident(name));
        }
        WorldKey::Interface(id) => {
            let iface = &resolve.interfaces[*id];
            let pkg = iface.package.unwrap();
            let pkgname = resolve.packages[pkg].name.clone();
            path.push(to_rust_ident(&pkgname.namespace));
            path.push(name_package_module(resolve, pkg));
            path.push(to_rust_ident(iface.name.as_ref().unwrap()));
        }
    }
    path
}

enum Identifier<'a> {
    World(WorldId),
    Interface(InterfaceId, &'a WorldKey),
}

/// Configuration for how interfaces are generated.
#[derive(Debug, Clone, Default)]
pub struct WithGeneration {
    /// How interface should be generated
    with: HashMap<String, WithOption>,
    /// Whether to generate interfaces not present in the `with` map
    pub generate_by_default: bool,
}

impl WithGeneration {
    fn iter(&self) -> impl Iterator<Item = (&String, &WithOption)> {
        self.with.iter()
    }

    pub fn extend(&mut self, with: HashMap<String, WithOption>) {
        self.with.extend(with);
    }
}

impl FromStr for WithGeneration {
    type Err = String;

    fn from_str(s: &str) -> std::prelude::v1::Result<Self, Self::Err> {
        let with = s
            .trim()
            .split(',')
            .map(|s| {
                let (k, v) = s.trim().split_once('=').ok_or_else(|| {
                    format!("expected string of form `<key>=<value>[,<key>=<value>...]`; got `{s}`")
                })?;
                let k = k.trim().to_string();
                let v = match v.trim() {
                    "generate" => WithOption::Generate,
                    v => WithOption::Path(v.to_string()),
                };
                Ok((k, v))
            })
            .collect::<Result<HashMap<_, _>, Self::Err>>()?;
        Ok(WithGeneration {
            with,
            generate_by_default: false,
        })
    }
}

/// Options for with "with" remappings.
#[derive(Debug, Clone)]
pub enum WithOption {
    Path(String),
    Generate,
}

impl std::fmt::Display for WithOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WithOption::Path(p) => f.write_fmt(format_args!("\"{p}\"")),
            WithOption::Generate => f.write_str("generate"),
        }
    }
}

impl From<WithOption> for InterfaceGeneration {
    fn from(opt: WithOption) -> Self {
        match opt {
            WithOption::Path(p) => InterfaceGeneration::Remap(p),
            WithOption::Generate => InterfaceGeneration::Generate,
        }
    }
}

#[derive(Default)]
struct FnSig {
    private: bool,
    use_item_name: bool,
    self_arg: Option<String>,
    self_is_first_param: bool,
}

#[must_use]
pub fn to_rust_ident(name: &str) -> String {
    match name {
        // Escape Rust keywords.
        // Source: https://doc.rust-lang.org/reference/keywords.html
        "as" => "as_".into(),
        "break" => "break_".into(),
        "const" => "const_".into(),
        "continue" => "continue_".into(),
        "crate" => "crate_".into(),
        "else" => "else_".into(),
        "enum" => "enum_".into(),
        "extern" => "extern_".into(),
        "false" => "false_".into(),
        "fn" => "fn_".into(),
        "for" => "for_".into(),
        "if" => "if_".into(),
        "impl" => "impl_".into(),
        "in" => "in_".into(),
        "let" => "let_".into(),
        "loop" => "loop_".into(),
        "match" => "match_".into(),
        "mod" => "mod_".into(),
        "move" => "move_".into(),
        "mut" => "mut_".into(),
        "new" => "new_".into(),
        "pub" => "pub_".into(),
        "ref" => "ref_".into(),
        "return" => "return_".into(),
        "self" => "self_".into(),
        "static" => "static_".into(),
        "struct" => "struct_".into(),
        "super" => "super_".into(),
        "trait" => "trait_".into(),
        "true" => "true_".into(),
        "type" => "type_".into(),
        "unsafe" => "unsafe_".into(),
        "use" => "use_".into(),
        "where" => "where_".into(),
        "while" => "while_".into(),
        "async" => "async_".into(),
        "await" => "await_".into(),
        "dyn" => "dyn_".into(),
        "abstract" => "abstract_".into(),
        "become" => "become_".into(),
        "box" => "box_".into(),
        "do" => "do_".into(),
        "final" => "final_".into(),
        "macro" => "macro_".into(),
        "override" => "override_".into(),
        "priv" => "priv_".into(),
        "typeof" => "typeof_".into(),
        "unsized" => "unsized_".into(),
        "virtual" => "virtual_".into(),
        "yield" => "yield_".into(),
        "try" => "try_".into(),
        s => s.to_snake_case(),
    }
}

fn to_upper_camel_case(name: &str) -> String {
    match name {
        // The name "Guest" is reserved for traits generated by exported
        // interfaces, so remap types defined in wit to something else.
        "guest" => "Guest_".to_string(),
        s => s.to_upper_camel_case(),
    }
}

fn int_repr(repr: Int) -> &'static str {
    match repr {
        Int::U8 => "u8",
        Int::U16 => "u16",
        Int::U32 => "u32",
        Int::U64 => "u64",
    }
}

enum RustFlagsRepr {
    U8,
    U16,
    U32,
    U64,
    U128,
}

impl RustFlagsRepr {
    fn new(f: &Flags) -> RustFlagsRepr {
        match f.repr() {
            FlagsRepr::U8 => RustFlagsRepr::U8,
            FlagsRepr::U16 => RustFlagsRepr::U16,
            FlagsRepr::U32(1) => RustFlagsRepr::U32,
            FlagsRepr::U32(2) => RustFlagsRepr::U64,
            FlagsRepr::U32(3 | 4) => RustFlagsRepr::U128,
            FlagsRepr::U32(n) => panic!("unsupported number of flags: {}", n * 32),
        }
    }
}

impl fmt::Display for RustFlagsRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RustFlagsRepr::U8 => "u8".fmt(f),
            RustFlagsRepr::U16 => "u16".fmt(f),
            RustFlagsRepr::U32 => "u32".fmt(f),
            RustFlagsRepr::U64 => "u64".fmt(f),
            RustFlagsRepr::U128 => "u128".fmt(f),
        }
    }
}
