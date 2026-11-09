use crate::config::StringList;
use crate::{Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{Context, Result, bail};
use clap::Parser;
use heck::ToSnakeCase;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Default, Debug, Clone, Parser)]
pub struct RustOpts {
    /// A custom `path` dependency to use for `wit-bindgen-wrpc`.
    #[clap(long, conflicts_with = "rust_wit_bindgen_version", value_name = "PATH")]
    rust_wit_bindgen_path: Option<PathBuf>,

    /// A custom version to use for the `wit-bindgen-wrpc` dependency.
    #[clap(long, conflicts_with = "rust_wit_bindgen_path", value_name = "X.Y.Z")]
    rust_wit_bindgen_version: Option<String>,

    /// A custom `path` dependency to use for `wrpc-transport`.
    #[clap(
        long,
        conflicts_with = "rust_wrpc_transport_version",
        value_name = "PATH"
    )]
    rust_wrpc_transport_path: Option<PathBuf>,

    /// A custom version to use for the `wrpc-transport` dependency.
    #[clap(
        long,
        conflicts_with = "rust_wrpc_transport_path",
        value_name = "X.Y.Z"
    )]
    rust_wrpc_transport_version: Option<String>,
}

/// Rust-specific configuration of component files
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RustConfig {
    /// Space-separated list or array of compiler flags to pass.
    #[serde(default)]
    rustflags: StringList,
    /// List of path to rust files to build as external crates and link to the
    /// main crate.
    #[serde(default)]
    externs: Vec<String>,
}

pub struct Rust;

#[derive(Default)]
pub struct State {
    /// Directory containing all compiled dependency rlibs (the `deps`
    /// subdirectory of the helper crate's target directory).
    deps_dir: PathBuf,
    /// Map from crate name to its compiled `.rlib`, captured from `cargo build
    /// --message-format=json`, used for exact `--extern` paths.
    rlibs: HashMap<String, PathBuf>,
}

impl LanguageMethods for Rust {
    fn display(&self) -> &str {
        "rust"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
    }

    fn should_fail_verify(
        &self,
        name: &str,
        _config: &crate::config::WitConfig,
        args: &[String],
    ) -> bool {
        // wRPC's Rust generator does not implement upstream's `--ownership`
        // and `--map-type` flags.
        if args.iter().any(|arg| {
            ["--ownership", "--map-type"]
                .iter()
                .any(|flag| arg.starts_with(flag))
        }) {
            return true;
        }
        // The wRPC Rust generator does not yet support `map`.
        name == "map.wit" || name == "map.wit-merge-equal"
    }

    fn should_fail_runtime(&self, name: &str) -> bool {
        name == "map"
    }

    fn should_fail_runtime_component(&self, name: &str, component: &str) -> bool {
        match (name, component) {
            // `--ownership` is not supported.
            (
                "ownership",
                "runner-owning" | "runner-borrowing" | "runner-borrowing-duplicate-if-necessary",
            ) => true,
            // `--enable-method-chaining` is not supported.
            ("method-chaining", "runner" | "test") => true,
            _ => self.should_fail_runtime(name),
        }
    }

    fn codegen_test_variants(&self) -> &[(&str, &[&str])] {
        // Mirrors upstream wit-bindgen's codegen variant matrix. Variants for
        // flags wRPC does not implement are marked as expected failures in
        // `should_fail_verify`; they are kept to track parity with upstream and
        // to surface the day a flag becomes supported.
        &[
            ("borrowed", &["--ownership=borrowing"]),
            (
                "borrowed-duplicate",
                &["--ownership=borrowing-duplicate-if-necessary"],
            ),
            ("merge-equal", &["--merge-structurally-equal-types"]),
            ("hashmap", &["--map-type=std::collections::HashMap"]),
        ]
    }

    fn default_bindgen_args(&self) -> &[&str] {
        &["--generate-all"]
    }

    fn default_bindgen_args_for_codegen(&self) -> &[&str] {
        &["--stubs"]
    }

    fn prepare(&self, runner: &mut Runner<'_>) -> Result<()> {
        let cwd = env::current_dir()?;
        let opts = &runner.opts.rust;

        let wit_bindgen_dep = match &opts.rust_wit_bindgen_path {
            Some(path) => format!("path = {:?}", cwd.join(path)),
            None => {
                let version = opts
                    .rust_wit_bindgen_version
                    .as_deref()
                    .unwrap_or(env!("CARGO_PKG_VERSION"));
                format!("version = \"{version}\"")
            }
        };
        let transport_dep = match &opts.rust_wrpc_transport_path {
            Some(path) => format!("path = {:?}", cwd.join(path)),
            None => {
                let version = opts
                    .rust_wrpc_transport_version
                    .as_deref()
                    .unwrap_or(env!("CARGO_PKG_VERSION"));
                format!("version = \"{version}\"")
            }
        };

        let dir = cwd.join(&runner.opts.artifacts).join("rust");
        let helper = dir.join("deps-crate");

        super::write_if_different(
            &helper.join("Cargo.toml"),
            &format!(
                r#"
[package]
name = "wrpc-test-rust-deps"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[lib]
path = "lib.rs"

[dependencies]
wit-bindgen-wrpc = {{ {wit_bindgen_dep} }}
wrpc-transport = {{ {transport_dep}, features = ["net"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread", "net", "sync", "io-util", "time"] }}
futures = {{ version = "0.3", default-features = false, features = ["std", "async-await"] }}
anyhow = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#,
            ),
        )?;
        super::write_if_different(&helper.join("lib.rs"), "")?;

        println!("Building wRPC test dependencies...");
        let output = Command::new("cargo")
            .current_dir(&helper)
            .arg("build")
            .arg("--message-format=json")
            .output()
            .context("failed to spawn `cargo build`")?;
        if !output.status.success() {
            bail!(
                "failed to build wRPC test dependencies:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Capture the `.rlib` path for each compiled crate so that runtime-test
        // drivers and codegen verification can pass exact `--extern` paths.
        let mut rlibs = HashMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value["reason"] != "compiler-artifact" {
                continue;
            }
            let Some(name) = value["target"]["name"].as_str() else {
                continue;
            };
            if let Some(files) = value["filenames"].as_array() {
                for file in files.iter().filter_map(|f| f.as_str()) {
                    if file.ends_with(".rlib") {
                        rlibs.insert(name.to_string(), PathBuf::from(file));
                    }
                }
            }
        }

        let deps_dir = helper.join("target/debug/deps");
        anyhow::ensure!(
            deps_dir.is_dir(),
            "dependency directory {deps_dir:?} was not produced"
        );

        runner.rust_state = Some(State { deps_dir, rlibs });
        Ok(())
    }

    fn verify(&self, runner: &Runner<'_>, verify: &Verify<'_>) -> Result<()> {
        let bindings = verify
            .bindings_dir
            .join(format!("{}.rs", verify.world.to_snake_case()));
        let test_edition = |edition: Edition| -> Result<()> {
            let mut cmd = runner.rustc(edition);
            cmd.arg(&bindings);
            runner.extern_arg(&mut cmd, "wit_bindgen_wrpc")?;
            cmd.arg("--crate-type=rlib")
                .arg("-o")
                .arg(verify.artifacts_dir.join("tmp"));
            runner.run_command(&mut cmd)?;
            Ok(())
        };

        test_edition(Edition::E2021)?;
        test_edition(Edition::E2024)?;
        Ok(())
    }
}

enum Edition {
    E2021,
    E2024,
}

/// The driver linking a `client` (the `runner` world) and its dependencies
/// (`server` for the first, then each named after its world) into one binary.
/// Each dependency is served over its own in-process TCP transport and receives
/// a client routing its imports to the following dependencies as its context.
const DRIVER: &str = r##"#![allow(warnings)]

use core::future::Future;
use core::net::{Ipv6Addr, SocketAddr};
use core::time::Duration;

use std::sync::{Arc, Mutex};

use anyhow::bail;
use futures::{Stream, StreamExt as _, stream};
use tokio::io::BufReader;
use tokio::net::TcpListener;
use wit_bindgen_wrpc::bytes::Bytes;
use wrpc_transport::frame::{Incoming, Outgoing, Server, tcp};
use wrpc_transport::{Invoke, Serve};

{modules}

struct Router(
    Vec<(
        Vec<(String, String)>,
        tcp::Client<SocketAddr>,
    )>,
);

impl Invoke for Router {
    type Context = ();

    async fn invoke<P>(
        &self,
        cx: (),
        instance: &str,
        func: &str,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let Some((.., clt)) = self
            .0
            .iter()
            .find(|(names, ..)| names.iter().any(|(i, f)| *i == instance && *f == func))
        else {
            bail!("no component exports `{instance}#{func}`")
        };
        clt.invoke(cx, instance, func, paths).await
    }
}

struct Recorder<'a, T>(&'a T, Mutex<Vec<(String, String)>>);

impl<'a, T: Serve> Serve for Recorder<'a, T> {
    type Context = T::Context;

    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: Arc<[Box<[Option<usize>]>]>,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Outgoing,
                    Incoming,
                )>,
            > + Send
            + 'static
            + use<'a, T>,
        >,
    > + Send {
        self.1
            .lock()
            .unwrap()
            .push((instance.to_string(), func.to_string()));
        self.0.serve(instance, func, paths)
    }
}

macro_rules! serve {
    ($routes:ident, $component:ident) => {{
        let lis = TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
            .await
            .expect("failed to start TCP listener");
        let addr = lis.local_addr().expect("failed to get server address");

        let srv = Arc::new(Server::default());

        // Register the exported handlers, then serve each invocation
        // concurrently in the background.
        let recorder = Recorder(srv.as_ref(), Default::default());
        let invocations = $component::serve(&recorder, $component::Component)
            .await
            .expect(concat!("failed to serve `", stringify!($component), "` exports"));
        let names = recorder.1.into_inner().unwrap();
        tokio::spawn(async move {
            let mut invocations = stream::select_all(invocations.into_iter().map(
                |(instance, name, invocations)| {
                    invocations.map(move |res| (instance, name, res))
                },
            ));
            while let Some((instance, name, invocation)) =
                invocations.next().await
            {
                let invocation = invocation.unwrap_or_else(|err| {
                    panic!("failed to accept `{instance}#{name}` invocation: {err:?}")
                });
                tokio::spawn(async move {
                    if let Err(err) = invocation.await {
                        eprintln!("failed to serve `{instance}#{name}` invocation: {err:?}");
                    }
                });
            }
        });

        // Accept connections forever in the background. Each client invocation
        // opens a new connection.
        let router = Arc::new(Router($routes.clone()));
        tokio::spawn(async move {
            loop {
                let (stream, _) = lis.accept().await.expect("failed to accept connection");
                let (rx, tx) = tcp::split(stream);
                let srv = Arc::clone(&srv);
                let router = Arc::clone(&router);
                tokio::spawn(async move {
                    if let Err(err) = srv.accept(router, tx, BufReader::new(rx)).await {
                        eprintln!("failed to accept connection: {err}");
                    }
                });
            }
        });
        $routes.insert(0, (names, tcp::Client::from(addr)));
    }};
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Fail the test on any panic, including in handlers running in spawned tasks.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        std::process::abort();
    }));

    let mut routes = Vec::default();
    {serve}

    // Run the `client` to completion.
    let clt = Router(routes);
    tokio::time::timeout(Duration::from_secs(10), client::run(&clt))
        .await
        .expect("runtime test timed out")?;
    Ok(())
}
"##;

/// Crates built during `prepare` that runtime-test crates are linked against.
const DEPS: [&str; 7] = [
    "wit_bindgen_wrpc",
    "wrpc_transport",
    "tokio",
    "futures",
    "anyhow",
    "serde",
    "serde_json",
];

impl Runner<'_> {
    fn rustc(&self, edition: Edition) -> Command {
        let state = self.rust_state.as_ref().unwrap();
        let mut cmd = Command::new("rustc");
        cmd.arg(match edition {
            Edition::E2021 => "--edition=2021",
            Edition::E2024 => "--edition=2024",
        })
        .arg("-Dwarnings")
        .arg("-Cdebuginfo=1")
        .arg("-L")
        .arg(format!("dependency={}", state.deps_dir.display()));
        cmd
    }

    /// Adds `--extern <crate>=<rlib>` to `cmd` using the exact rlib path
    /// captured during `prepare`.
    fn extern_arg(&self, cmd: &mut Command, krate: &str) -> Result<()> {
        let state = self.rust_state.as_ref().unwrap();
        let rlib = state
            .rlibs
            .get(krate)
            .with_context(|| format!("no compiled rlib found for crate `{krate}`"))?;
        cmd.arg("--extern")
            .arg(format!("{krate}={}", rlib.display()));
        Ok(())
    }

    /// Builds and runs a single Rust runtime test by linking the `runner` and
    /// its dependencies into one binary connected over in-process TCP
    /// transports.
    pub(crate) fn rust_runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        runner_bindings: &std::path::Path,
        deps: &[&(&Component, PathBuf)],
    ) -> Result<()> {
        let mut name = runner.name.clone();
        let mut components = vec![("client".to_string(), runner, runner_bindings)];
        for (i, (dep, bindings)) in deps.iter().enumerate() {
            name.push('-');
            name.push_str(&dep.name);
            let module = if i == 0 {
                "server".to_string()
            } else {
                dep.bindgen.world.to_snake_case()
            };
            components.push((module, *dep, bindings.as_path()));
        }
        let artifacts_dir = env::current_dir()?
            .join(&self.opts.artifacts)
            .join(&case.name)
            .join(name);
        fs::create_dir_all(&artifacts_dir)
            .with_context(|| format!("failed to create directory {artifacts_dir:?}"))?;

        let mut modules = String::default();
        let mut rustflags = Vec::default();
        let mut externs = Vec::default();
        for (module, component, bindings) in &components {
            let config = component.deserialize_lang_config::<RustConfig>()?;
            let flags = Vec::from(config.rustflags);
            let src = component.path.canonicalize()?;
            let manifest_dir = src.parent().unwrap();
            for file in &config.externs {
                let file = manifest_dir.join(file);
                let stem = file.file_stem().unwrap().to_str().unwrap();
                let name = stem.replace('-', "_");
                let rlib = artifacts_dir.join(format!("lib{name}.rlib"));
                let mut cmd = self.rustc(Edition::E2021);
                cmd.arg(&file)
                    .arg("--crate-type=rlib")
                    .arg("--crate-name")
                    .arg(&name)
                    .arg("-o")
                    .arg(&rlib)
                    .args(&flags)
                    .env("CARGO_MANIFEST_DIR", manifest_dir);
                for krate in DEPS {
                    self.extern_arg(&mut cmd, krate)?;
                }
                self.run_command(&mut cmd)
                    .with_context(|| format!("failed to compile extern crate {file:?}"))?;
                externs.push(format!("{name}={}", rlib.display()));
            }
            rustflags.extend(flags);
            let bindings = bindings.join(format!("{}.rs", component.bindgen.world.to_snake_case()));
            modules.push_str(&format!(
                "pub mod {module} {{\n    include!({bindings:?});\n    include!({src:?});\n}}\n\n",
            ));
        }
        let serve = components[1..]
            .iter()
            .rev()
            .map(|(module, ..)| format!("serve!(routes, {module});"))
            .collect::<Vec<_>>()
            .join("\n    ");

        let driver = artifacts_dir.join("driver.rs");
        super::write_if_different(
            &driver,
            DRIVER
                .replace("{modules}", modules.trim_end())
                .replace("{serve}", &serve),
        )?;

        let runner_path = runner.path.canonicalize()?;
        let bin = artifacts_dir.join("driver");
        let mut cmd = self.rustc(Edition::E2021);
        cmd.arg(&driver)
            .arg("--crate-type=bin")
            .arg("-o")
            .arg(&bin)
            .args(&rustflags)
            // Set so a `wit_bindgen_wrpc::generate!` embedded in a runner/test
            // source (e.g. the `with` tests' `mod other`) resolves its `path`
            // relative to the test directory, like upstream.
            .env("CARGO_MANIFEST_DIR", runner_path.parent().unwrap());
        for extern_ in DEPS {
            self.extern_arg(&mut cmd, extern_)?;
        }
        for extern_ in &externs {
            cmd.arg("--extern").arg(extern_);
        }
        self.run_command(&mut cmd)
            .context("failed to compile runtime test driver")?;

        self.run_command(&mut Command::new(&bin))
            .context("runtime test driver failed")
    }
}
