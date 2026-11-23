use crate::interface::InterfaceGenerator;
use anyhow::{Result, bail};
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::mem;
use wit_bindgen_core::wit_parser::{
    Flags, FlagsRepr, Function, Int, InterfaceId, Resolve, SizeAlign, TypeId, TypeOwner, WorldId,
    WorldItem, WorldKey,
};
use wit_bindgen_core::{
    Files, InterfaceGenerator as _, Source, Types, WorldGenerator, dealias, name_package_module,
    uwrite, uwriteln,
};
use wrpc_introspect::uses_error_context;

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
    src_preamble: Source,
    src: Source,
    opts: Opts,
    import_modules: Vec<(String, Vec<String>)>,
    export_modules: Vec<(String, Vec<String>)>,
    skip: HashSet<String>,
    interface_names: HashMap<InterfaceId, InterfaceName>,
    import_funcs_called: bool,
    with_name_counter: usize,
    // Track which interfaces and types are generated. Remapped interfaces and types provided via `with`
    // are required to be used.
    generated_types: HashSet<String>,
    world: Option<WorldId>,

    export_paths: Vec<String>,
    /// Arities of the generated `TupleN` structs
    tuples: BTreeSet<usize>,
    /// Maps wit interface and type names to their Rust identifiers
    with: GenerationConfiguration,
}

#[derive(Default)]
struct GenerationConfiguration {
    map: HashMap<String, TypeGeneration>,
    generate_by_default: bool,
}

impl GenerationConfiguration {
    fn get(&self, key: &str) -> Option<&TypeGeneration> {
        self.map.get(key).or_else(|| {
            self.generate_by_default
                .then_some(&TypeGeneration::Generate)
        })
    }

    fn insert(&mut self, name: String, generate: TypeGeneration) {
        self.map.insert(name, generate);
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &TypeGeneration)> {
        self.map.iter()
    }
}

/// How a wit interface or type should be rendered in Rust
enum TypeGeneration {
    /// Uses a Rust identifier defined elsewhere
    Remap(String),
    /// Define the interface or type with this bindgen invocation
    Generate,
}

impl TypeGeneration {
    /// Returns true if the interface or type should be defined with this bindgen invocation
    fn generated(&self) -> bool {
        match self {
            TypeGeneration::Generate => true,
            TypeGeneration::Remap(_) => false,
        }
    }
}

#[cfg(feature = "clap")]
fn parse_with(s: &str) -> Result<(String, WithOption), String> {
    let (k, v) = s.split_once('=').ok_or_else(|| {
        format!("expected string of form `<key>=<value>[,<key>=<value>...]`; got `{s}`")
    })?;
    let v = match v {
        "generate" => WithOption::Generate,
        other => WithOption::Path(other.to_string()),
    };
    Ok((k.to_string(), v))
}

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize),
    serde(default, rename_all = "kebab-case")
)]
pub struct Opts {
    /// Whether or not a formatter is executed to format generated code.
    #[cfg_attr(feature = "clap", arg(long))]
    pub format: bool,

    /// If true, code generation should pass borrowed string arguments as
    /// `&[u8]` instead of `&str`. Strings are still required to be valid
    /// UTF-8, but this avoids the need for Rust code to do its own UTF-8
    /// validation if it doesn't already have a `&str`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub raw_strings: bool,

    /// Names of functions to skip generating bindings for.
    #[cfg_attr(feature = "clap", arg(long, value_name = "NAME"))]
    pub skip: Vec<String>,

    /// If true, generate stub implementations for any exported functions,
    /// interfaces, and/or resources.
    #[cfg_attr(feature = "clap", arg(long))]
    pub stubs: bool,

    /// The optional path to the map type to use for WIT `map<K, V>`.
    ///
    /// The specified type must accept two type parameters `<K, V>`, implement
    /// `wrpc_transport::Encode` and `wrpc_transport::Decode` and provide the
    /// `BTreeMap` API used by the generated code (e.g. `Debug`, `iter` and
    /// `FromIterator`).
    ///
    /// Defaults to `::std::collections::BTreeMap`.
    #[cfg_attr(feature = "clap", arg(long, value_name = "PATH"))]
    pub map_type: Option<String>,

    /// The optional path to the bitflags crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::bitflags`.
    #[cfg_attr(feature = "clap", arg(long))]
    pub bitflags_path: Option<String>,

    /// Additional derive attributes to add to generated types. If using in a CLI, this flag can be
    /// specified multiple times to add multiple attributes.
    ///
    /// These derive attributes will be added to any generated structs or enums
    #[cfg_attr(feature = "clap", arg(long, short = 'd', value_name = "DERIVE"))]
    pub additional_derive_attributes: Vec<String>,

    /// Variants and records to ignore when applying additional derive attributes.
    ///
    /// These names are specified as they are listed in the wit file, i.e. in kebab case.
    /// This feature allows some variants and records to use types for which adding traits will cause
    /// compilation to fail, such as serde::Deserialize on wasi:io/streams.
    ///
    #[cfg_attr(feature = "clap", arg(long, value_name = "NAME"))]
    pub additional_derive_ignore: Vec<String>,

    /// Remapping of wit import interface and type names to Rust module names
    /// and types.
    ///
    /// Argument must be of the form `k=v` and this option can be passed
    /// multiple times or one option can be comma separated, for example
    /// `k1=v1,k2=v2`.
    #[cfg_attr(feature = "clap", arg(long, value_parser = parse_with, value_delimiter = ','))]
    pub with: Vec<(String, WithOption)>,

    /// Indicates that all interfaces not specified in `with` should be
    /// generated.
    #[cfg_attr(feature = "clap", arg(long))]
    pub generate_all: bool,

    /// Whether to generate unused structures, not generated by default (false)
    #[cfg_attr(feature = "clap", arg(long))]
    pub generate_unused_types: bool,

    /// Find all structurally equal types and only generate one type definition
    /// for each equivalence class.
    ///
    /// Other types in the same class will be type aliases to the generated
    /// type. This avoids clone when converting between types that are
    /// structurally equal, which is useful when import and export the same
    /// interface.
    #[cfg_attr(
        feature = "clap",
        arg(long, require_equals = true, value_name = "true|false")
    )]
    pub merge_structurally_equal_types: Option<Option<bool>>,

    /// The optional path to the `anyhow` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::anyhow`.
    #[cfg_attr(feature = "clap", arg(long, value_name = "PATH"))]
    pub anyhow_path: Option<String>,

    /// The optional path to the `bytes` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::bytes`.
    #[cfg_attr(feature = "clap", arg(long, value_name = "PATH"))]
    pub bytes_path: Option<String>,

    /// The optional path to the `futures` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::futures`.
    #[cfg_attr(feature = "clap", arg(long, value_name = "PATH"))]
    pub futures_path: Option<String>,

    /// The optional path to the `tokio-util` crate to use.
    ///
    /// This defaults to `wit_bindgen_wrpc::tokio_util`.
    #[cfg_attr(feature = "clap", arg(long, value_name = "PATH"))]
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
}

impl Opts {
    #[must_use]
    pub fn build(self) -> Box<dyn WorldGenerator> {
        let mut r = RustWrpc::new();
        r.skip = self.skip.iter().cloned().collect();
        r.opts = self;
        Box::new(r)
    }

    fn merge_structurally_equal_types(&self) -> bool {
        const DEFAULT: bool = false;
        match self.merge_structurally_equal_types {
            // no option passed, use the default
            None => DEFAULT,
            // --merge-structurally-equal-types
            Some(None) => true,
            // --merge-structurally-equal-types=val
            Some(Some(val)) => val,
        }
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
            r#gen: self,
            resolve,
            decoding: false,
            encoding: false,
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

        emit(&mut self.src, map, &self.opts, true);
        fn emit(me: &mut Source, module: Module, opts: &Opts, toplevel: bool) {
            for (name, submodule) in module.submodules {
                if toplevel {
                    // Disable rustfmt. By default we already format the code
                    // using prettyplease, so we don't want `cargo fmt` to create
                    // extra diffs for users to deal with.
                    if opts.format {
                        uwriteln!(me, "#[rustfmt::skip]");
                    }

                    // Ignore dead-code and clippy warnings. If the bindings are
                    // only used within a crate, and not exported to a different
                    // crate, some parts may be unused, and that's ok.
                    uwriteln!(me, "#[allow(dead_code, clippy::all)]");
                }

                uwriteln!(me, "pub mod {name} {{");
                emit(me, submodule, opts, false);
                uwriteln!(me, "}}");
            }
            for submodule in module.contents {
                uwriteln!(me, "{submodule}");
            }
        }
    }

    fn name_interface(
        &mut self,
        resolve: &Resolve,
        id: InterfaceId,
        name: &WorldKey,
        is_export: bool,
    ) -> Result<bool> {
        let with_name = resolve.name_world_key(name);
        // `with` remappings only apply to imports; exports are always generated
        // since their types are defined by the implementation.
        let remapping = if is_export {
            &TypeGeneration::Generate
        } else {
            let Some(remapping) = self.with.get(&with_name) else {
                bail!("no remapping found for {with_name:?} - use the `generate!` macro's `with` option to force the interface to be generated or specify where it is already defined:
```
with: {{\n\t{with_name:?}: generate\n}}
```")
            };
            remapping
        };
        self.generated_types.insert(with_name);
        let entry = match remapping {
            TypeGeneration::Remap(remapped_path) => {
                let name = format!("__with_name{}", self.with_name_counter);
                self.with_name_counter += 1;
                uwriteln!(
                    self.src,
                    "#[allow(unfulfilled_lint_expectations, unused_imports)]"
                );
                uwriteln!(self.src, "use {remapped_path} as {name};");
                InterfaceName {
                    remapped: true,
                    path: name,
                }
            }
            TypeGeneration::Generate => {
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

    fn anyhow_path(&self) -> &str {
        self.opts
            .anyhow_path
            .as_deref()
            .unwrap_or("::wit_bindgen_wrpc::anyhow")
    }

    fn map_type_path(&self) -> &str {
        self.opts
            .map_type
            .as_deref()
            .unwrap_or("::std::collections::BTreeMap")
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

    /// Generates the `handle` function for the `world_id` specified, which dispatches to the
    /// `lookup__` and `serve__` functions collected in `self.export_paths` prior.
    fn finish_handle_function(&mut self) {
        if self.export_paths.is_empty() {
            return;
        }
        let paths: Vec<_> = self
            .export_paths
            .iter()
            .map(|path| {
                if path.is_empty() {
                    String::default()
                } else {
                    format!("{path}::")
                }
            })
            .collect();
        let bound = paths
            .iter()
            .map(|path| format!("{path}Handler"))
            .collect::<Vec<_>>()
            .join(" + ");
        let anyhow = self.anyhow_path().to_string();
        let wrpc_transport = self.wrpc_transport_path().to_string();
        uwrite!(
            self.src,
            r#"
pub fn handle<S, H>(
    handler: &H,
    instance: &str,
    func: &str,
    srv: S,
) -> ::core::result::Result<
    impl ::core::future::Future<Output = {anyhow}::Result<()>> + ::core::marker::Send + use<S, H>,
    S,
>
where
    S: {wrpc_transport}::Serve + ::core::marker::Send,
    H: {bound} + ::core::marker::Send + ::core::marker::Sync + ::core::clone::Clone,
{{
    let (i, func) = "#
        );
        for (i, path) in paths.iter().enumerate() {
            uwrite!(
                self.src,
                r#"if let ::core::option::Option::Some(func) = {path}lookup__(instance, func) {{
        ({i}, func)
    }} else "#
            );
        }
        uwrite!(
            self.src,
            r#"{{
        return ::core::result::Result::Err(srv);
    }};
    let handler = ::core::clone::Clone::clone(handler);
    ::core::result::Result::Ok(async move {{
        match i {{"#
        );
        for (i, path) in paths.iter().enumerate() {
            uwrite!(
                self.src,
                r#"
            {i} => {path}serve__(handler, func, srv).await,"#
            );
        }
        uwriteln!(
            self.src,
            r#"
            _ => ::core::unreachable!(),
        }}
    }})
}}"#
        );
    }
}

impl RustWrpc {
    /// Generates the `TupleN` structs used for tuples, which are too long for Rust tuple trait
    /// implementations
    fn finish_tuples(&mut self) {
        const DERIVES: [&str; 9] = [
            "::core::clone::Clone",
            "::core::marker::Copy",
            "::core::fmt::Debug",
            "::core::default::Default",
            "::core::cmp::PartialEq",
            "::core::cmp::Eq",
            "::core::cmp::PartialOrd",
            "::core::cmp::Ord",
            "::core::hash::Hash",
        ];
        let derives = DERIVES
            .iter()
            .map(ToString::to_string)
            .chain(
                self.opts
                    .additional_derive_attributes
                    .iter()
                    .filter(|derive| {
                        let name = derive.rsplit("::").next().unwrap_or(derive).trim();
                        !DERIVES
                            .iter()
                            .any(|std| std.rsplit("::").next() == Some(name))
                    })
                    .cloned(),
            )
            .collect::<Vec<_>>()
            .join(", ");
        let bytes = self.bytes_path().to_string();
        let tokio_util = self.tokio_util_path().to_string();
        let wrpc_transport = self.wrpc_transport_path().to_string();
        for n in mem::take(&mut self.tuples) {
            let name = format!("Tuple{n}");
            let ts = (0..n).map(|i| format!("T{i}, ")).collect::<String>();
            let fields = (0..n).map(|i| format!("pub T{i}, ")).collect::<String>();
            let vs = (0..n).map(|i| format!("v{i}, ")).collect::<String>();
            let encode = (0..n)
                .map(|i| format!("T{i}: {wrpc_transport}::Encode, "))
                .collect::<String>();
            let decode = (0..n)
                .map(|i| format!("T{i}: {wrpc_transport}::Decode, "))
                .collect::<String>();
            let encode_ref = (0..n)
                .map(|i| {
                    format!(
                        "T{i}: {wrpc_transport}::Encode, T{i}::Encoder: {tokio_util}::codec::Encoder<&'a T{i}, Error = ::std::io::Error>, "
                    )
                })
                .collect::<String>();
            let decode_static = (0..n)
                .map(|i| {
                    format!("T{i}: {wrpc_transport}::Decode + ::core::marker::Send + 'static, ")
                })
                .collect::<String>();
            let encoders = (0..n)
                .map(|i| format!("T{i}::Encoder, "))
                .collect::<String>();
            let cs = (0..n).map(|i| format!("C{i}, ")).collect::<String>();
            let vs_ty = (0..n).map(|i| format!("V{i}, ")).collect::<String>();
            let transmit_cs = (0..n)
                .map(|i| format!("C{i}: {wrpc_transport}::Transmit, "))
                .collect::<String>();
            let encoder_cs = (0..n)
                .map(|i| {
                    format!("C{i}: {tokio_util}::codec::Encoder<V{i}, Error = ::std::io::Error>, ")
                })
                .collect::<String>();
            let encoder_ref_cs = (0..n)
                .map(|i| {
                    format!(
                        "C{i}: {tokio_util}::codec::Encoder<&'a V{i}, Error = ::std::io::Error>, "
                    )
                })
                .collect::<String>();
            let transmit = (0..n)
                .map(|i| {
                    format!(
                        r"
            path.push({i});
            let res = {wrpc_transport}::Transmit::poll_transmit(&mut self.{i}, cx, path, dst);
            path.pop();
            match res {{
                ::core::task::Poll::Ready(Ok(())) => {{}}
                ::core::task::Poll::Ready(Err(err)) => return ::core::task::Poll::Ready(Err(err)),
                ::core::task::Poll::Pending => ready = false,
            }}"
                    )
                })
                .collect::<String>();
            let transmit_closed = (0..n)
                .map(|i| format!("{wrpc_transport}::Transmit::is_closed(&self.{i})"))
                .collect::<Vec<_>>()
                .join(" && ");
            let encode_fields = (0..n)
                .map(|i| {
                    format!("{tokio_util}::codec::Encoder::encode(&mut self.{i}, v{i}, dst)?;")
                })
                .collect::<String>();
            let decoder_fields = (0..n)
                .map(|i| format!("c{i}: T{i}::Decoder, v{i}: ::core::option::Option<T{i}>, "))
                .collect::<String>();
            let decoder_defaults = (0..n)
                .map(|i| {
                    format!(
                        "c{i}: ::core::default::Default::default(), v{i}: ::core::option::Option::None, "
                    )
                })
                .collect::<String>();
            let receive = (0..n)
                .map(|i| {
                    format!(
                        "::core::option::Option::Some(({i}, rest)) => {wrpc_transport}::Receive::poll_receive(&mut self.c{i}, cx, rest, src),"
                    )
                })
                .collect::<String>();
            let receive_closed = (0..n)
                .map(|i| format!("{wrpc_transport}::Receive::is_closed(&self.c{i})"))
                .collect::<Vec<_>>()
                .join(" && ");
            let decode_fields = (0..n)
                .map(|i| {
                    format!(
                        r"
            if self.v{i}.is_none() {{
                let ::core::option::Option::Some(v) = {tokio_util}::codec::Decoder::decode(&mut self.c{i}, src)? else {{
                    return ::core::result::Result::Ok(::core::option::Option::None);
                }};
                self.v{i} = ::core::option::Option::Some(v);
            }}"
                    )
                })
                .collect::<String>();
            let decoded = (0..n)
                .map(|i| format!("self.v{i}.take().unwrap(), "))
                .collect::<String>();
            let module = name.to_snake_case();
            uwriteln!(
                self.src,
                r"
#[derive({derives})]
pub struct {name}<{ts}>({fields});

impl<{encode}> {wrpc_transport}::Encode for {name}<{ts}> {{
    type Encoder = {module}__::Encoder<{encoders}>;
}}

impl<'a, {ts}> {wrpc_transport}::Encode for &'a {name}<{ts}>
where
    {encode_ref}
{{
    type Encoder = {module}__::Encoder<{encoders}>;
}}

impl<{decode_static}> {wrpc_transport}::Decode for {name}<{ts}> {{
    type Decoder = {module}__::Decoder<{ts}>;
    type ListDecoder = {wrpc_transport}::ListDecoder<Self::Decoder>;
}}

mod {module}__ {{
    #[derive(::core::default::Default)]
    pub struct Encoder<{cs}>({cs});

    impl<{transmit_cs}> {wrpc_transport}::Transmit for Encoder<{cs}> {{
        fn poll_transmit(&mut self, cx: &mut ::core::task::Context<'_>, path: &mut ::std::vec::Vec<usize>, dst: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            let mut ready = true;{transmit}
            if ready {{
                ::core::task::Poll::Ready(Ok(()))
            }} else {{
                ::core::task::Poll::Pending
            }}
        }}

        fn is_closed(&self) -> bool {{
            {transmit_closed}
        }}
    }}

    impl<{cs} {vs_ty}> {tokio_util}::codec::Encoder<super::{name}<{vs_ty}>> for Encoder<{cs}>
    where
        {encoder_cs}
    {{
        type Error = ::std::io::Error;

        fn encode(&mut self, super::{name}({vs}): super::{name}<{vs_ty}>, dst: &mut {bytes}::BytesMut) -> ::std::io::Result<()> {{
            {encode_fields}
            Ok(())
        }}
    }}

    impl<'a, {cs} {vs_ty}> {tokio_util}::codec::Encoder<&'a super::{name}<{vs_ty}>> for Encoder<{cs}>
    where
        {encoder_ref_cs}
    {{
        type Error = ::std::io::Error;

        fn encode(&mut self, super::{name}({vs}): &'a super::{name}<{vs_ty}>, dst: &mut {bytes}::BytesMut) -> ::std::io::Result<()> {{
            {encode_fields}
            Ok(())
        }}
    }}

    pub struct Decoder<{decode}> {{
        {decoder_fields}
    }}

    impl<{decode}> ::core::default::Default for Decoder<{ts}> {{
        fn default() -> Self {{
            Self {{ {decoder_defaults} }}
        }}
    }}

    impl<{decode}> {wrpc_transport}::Receive for Decoder<{ts}> {{
        fn poll_receive(&mut self, cx: &mut ::core::task::Context<'_>, path: &[usize], src: &mut {bytes}::BytesMut) -> ::core::task::Poll<::std::io::Result<()>> {{
            match path.split_first() {{
                {receive}
                _ => ::core::task::Poll::Ready(::core::result::Result::Err({wrpc_transport}::error_unknown_path(path))),
            }}
        }}

        fn is_closed(&self) -> bool {{
            {receive_closed}
        }}
    }}

    impl<{decode}> {tokio_util}::codec::Decoder for Decoder<{ts}> {{
        type Item = super::{name}<{ts}>;
        type Error = ::std::io::Error;

        fn decode(&mut self, src: &mut {bytes}::BytesMut) -> ::core::result::Result<::core::option::Option<Self::Item>, Self::Error> {{{decode_fields}
            ::core::result::Result::Ok(::core::option::Option::Some(super::{name}({decoded})))
        }}
    }}
}}"
            );
        }
    }
}

impl WorldGenerator for RustWrpc {
    fn preprocess(&mut self, resolve: &Resolve, world: WorldId) -> Result<()> {
        if uses_error_context(resolve, world) {
            bail!("`error-context` is not supported");
        }
        wit_bindgen_core::generated_preamble(&mut self.src_preamble, env!("CARGO_PKG_VERSION"));

        // Render some generator options to assist with debugging and/or to help
        // recreate it if the original generation command is lost.
        uwriteln!(self.src_preamble, "// Options used:");
        if self.opts.raw_strings {
            uwriteln!(self.src_preamble, "//   * raw_strings");
        }
        if !self.opts.skip.is_empty() {
            uwriteln!(self.src_preamble, "//   * skip: {:?}", self.opts.skip);
        }
        if self.opts.stubs {
            uwriteln!(self.src_preamble, "//   * stubs");
        }
        if let Some(map_type) = &self.opts.map_type {
            uwriteln!(self.src_preamble, "//   * map_type: {:?}", map_type);
        }
        if !self.opts.additional_derive_attributes.is_empty() {
            uwriteln!(
                self.src_preamble,
                "//   * additional derives {:?}",
                self.opts.additional_derive_attributes
            );
        }
        if !self.opts.additional_derive_ignore.is_empty() {
            uwriteln!(
                self.src_preamble,
                "//   * additional derives ignored {:?}",
                self.opts.additional_derive_ignore
            );
        }
        for (k, v) in &self.opts.with {
            uwriteln!(self.src_preamble, "//   * with {k:?} = {v}");
        }
        if self.opts.merge_structurally_equal_types() {
            uwriteln!(self.src_preamble, "//   * merge_structurally_equal_types");
        }
        self.types.analyze(resolve);
        if self.opts.merge_structurally_equal_types() {
            self.types.collect_equal_types(resolve, world, &|_| true);
        }
        self.world = Some(world);

        let world = &resolve.worlds[world];
        // Specify that all imports local to the world's package should be generated
        for (key, item) in world.imports.iter() {
            if let WorldItem::Interface { id, .. } = item
                && resolve.interfaces[*id].package == world.package
            {
                let name = resolve.name_world_key(key);
                if self.with.get(&name).is_none() {
                    self.with.insert(name, TypeGeneration::Generate);
                }
            }
        }
        for (k, v) in &self.opts.with {
            self.with.insert(k.clone(), v.clone().into());
        }
        self.with.generate_by_default = self.opts.generate_all;
        for (key, item) in world.imports.iter() {
            if let WorldItem::Interface { id, .. } = item {
                self.name_interface(resolve, *id, key, false)?;
            }
        }
        for (key, item) in world.exports.iter() {
            if let WorldItem::Interface { id, .. } = item {
                self.name_interface(resolve, *id, key, true)?;
            }
        }
        Ok(())
    }

    fn import_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let mut to_define = Vec::new();
        for (name, ty_id) in resolve.interfaces[id].types.iter() {
            let full_name = full_wit_type_name(resolve, *ty_id);
            if let Some(type_gen) = self.with.get(&full_name) {
                // skip type definition generation for remapped types
                if type_gen.generated() {
                    to_define.push((name, ty_id));
                }
            } else {
                to_define.push((name, ty_id));
            }
            self.generated_types.insert(full_name);
        }

        let mut r#gen = self.interface(Identifier::Interface(id, name), resolve, true);
        let (snake, module_path) = r#gen.start_append_submodule(name);
        if r#gen.r#gen.interface_names[&id].remapped {
            return Ok(());
        }

        for (name, ty_id) in to_define {
            r#gen.define_type(name, *ty_id);
        }

        let instance = resolve.name_canonicalized_world_key(name);
        r#gen.generate_imports(&instance, resolve.interfaces[id].functions.values());

        let docs = &resolve.interfaces[id].docs;

        r#gen.finish_append_submodule(&snake, module_path, docs);

        Ok(())
    }

    fn import_funcs(
        &mut self,
        resolve: &Resolve,
        _world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) {
        self.import_funcs_called = true;

        let mut r#gen = self.interface(Identifier::World, resolve, true);
        r#gen.generate_imports("", funcs.iter().map(|(_, func)| *func));

        let src = r#gen.finish();
        self.src.push_str(&src);
    }

    fn export_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let mut to_define = Vec::new();
        for (name, ty_id) in resolve.interfaces[id].types.iter() {
            let full_name = full_wit_type_name(resolve, *ty_id);
            // `with` remappings only apply to imports; exported types are always generated.
            to_define.push((name, ty_id));
            self.generated_types.insert(full_name);
        }

        let mut r#gen = self.interface(Identifier::Interface(id, name), resolve, false);
        let (snake, module_path) = r#gen.start_append_submodule(name);
        if r#gen.r#gen.interface_names[&id].remapped {
            return Ok(());
        }

        for (name, ty_id) in to_define {
            r#gen.define_type(name, *ty_id);
        }

        let exports = r#gen.generate_exports(
            Identifier::Interface(id, name),
            resolve.interfaces[id].functions.values(),
        );

        let docs = &resolve.interfaces[id].docs;

        r#gen.finish_append_submodule(&snake, module_path, docs);
        if exports {
            self.export_paths
                .push(self.interface_names[&id].path.clone());
        }

        if self.opts.stubs {
            let mut r#gen = self.interface(Identifier::World, resolve, false);
            r#gen.generate_stub(Some(id), resolve.interfaces[id].functions.values());
            let stub = r#gen.finish();
            self.src.push_str(&stub);
        }
        Ok(())
    }

    fn export_funcs(
        &mut self,
        resolve: &Resolve,
        _world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) -> Result<()> {
        let mut r#gen = self.interface(Identifier::World, resolve, false);
        let exports = r#gen.generate_exports(Identifier::World, funcs.iter().map(|f| f.1));
        let src = r#gen.finish();
        self.src.push_str(&src);
        if exports {
            self.export_paths.push(String::new());
        }

        if self.opts.stubs {
            let mut r#gen = self.interface(Identifier::World, resolve, false);
            r#gen.generate_stub(None, funcs.iter().map(|f| f.1));
            let stub = r#gen.finish();
            self.src.push_str(&stub);
        }
        Ok(())
    }

    fn import_types(
        &mut self,
        resolve: &Resolve,
        _world: WorldId,
        types: &[(&str, TypeId)],
        _files: &mut Files,
    ) {
        let mut to_define = Vec::new();
        for (name, ty_id) in types {
            let full_name = full_wit_type_name(resolve, *ty_id);
            if let Some(type_gen) = self.with.get(&full_name) {
                // skip type definition generation for remapped types
                if type_gen.generated() {
                    to_define.push((name, ty_id));
                }
            } else {
                to_define.push((name, ty_id));
            }
            self.generated_types.insert(full_name);
        }
        let mut r#gen = self.interface(Identifier::World, resolve, true);
        for (name, ty) in to_define {
            r#gen.define_type(name, *ty);
        }
        let src = r#gen.finish();
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

        self.finish_handle_function();
        self.finish_tuples();

        if self.opts.stubs {
            self.src.push_str(
                "\n#[derive(::core::clone::Clone, ::core::fmt::Debug)]\npub struct Stub;\n",
            );
        }

        let mut src = mem::take(&mut self.src);
        if self.opts.format {
            let syntax_tree = syn::parse_file(src.as_str()).unwrap();
            *src.as_mut_string() = prettyplease::unparse(&syntax_tree);
        }

        // Prepend the preamble. We do this after formatting because
        // `syn::parse_file` + `prettyplease::unparse` does not preserve comments.
        let src_preamble = mem::take(&mut self.src_preamble);
        *src.as_mut_string() = format!("{}{}", src_preamble.as_str(), src.as_str());

        let module_name = name.to_snake_case();
        files.push(&format!("{module_name}.rs"), src.as_bytes());

        let remapped_keys = self
            .with
            .iter()
            .map(|(k, _)| k)
            .cloned()
            .collect::<HashSet<String>>();

        let mut unused_keys = remapped_keys
            .difference(&self.generated_types)
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
            path.push(to_rust_ident(&name_package_module(resolve, pkg)));
            path.push(to_rust_ident(iface.name.as_ref().unwrap()));
        }
    }
    path
}

enum Identifier<'a> {
    World,
    Interface(InterfaceId, &'a WorldKey),
}

/// Options for with "with" remappings.
#[derive(Debug, Clone)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize),
    serde(rename_all = "kebab-case")
)]
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

impl From<WithOption> for TypeGeneration {
    fn from(opt: WithOption) -> Self {
        match opt {
            WithOption::Path(p) => TypeGeneration::Remap(p),
            WithOption::Generate => TypeGeneration::Generate,
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
        // The name "Handler" is reserved for traits generated by exported
        // interfaces, so remap types defined in wit to something else.
        "handler" => "Handler_".to_string(),
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

#[derive(Debug, Clone)]
pub struct MissingWith(pub String);

impl fmt::Display for MissingWith {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "missing `with` mapping for the key `{}`", self.0)
    }
}

impl std::error::Error for MissingWith {}

/// Returns the full WIT type name with fully qualified interface name
fn full_wit_type_name(resolve: &Resolve, id: TypeId) -> String {
    let id = dealias(resolve, id);
    let type_def = &resolve.types[id];
    let interface_name = match type_def.owner {
        TypeOwner::World(w) => Some(resolve.worlds[w].name.clone()),
        TypeOwner::Interface(id) => resolve.id_of(id),
        TypeOwner::None => None,
    };
    match interface_name {
        Some(interface_name) => format!("{}/{}", interface_name, type_def.name.clone().unwrap()),
        None => type_def.name.clone().unwrap(),
    }
}

// bail!("no remapping found for {with_name:?} - use the `generate!` macro's `with` option to force the interface to be generated or specify where it is already defined:
// ```
// with: {{\n\t{with_name:?}: generate\n}}
// ```")
