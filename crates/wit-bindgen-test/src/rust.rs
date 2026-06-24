use crate::{Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{Context, Result, bail};
use clap::Parser;
use heck::ToSnakeCase;
use std::collections::HashMap;
use std::env;
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

    /// A custom `path` dependency to use as a `[patch.crates-io]` override for
    /// `leb128-tokio` (the LEB128 codec used transitively via `wasm-tokio`).
    ///
    /// Useful while a fix is pending an upstream release.
    #[clap(long, value_name = "PATH")]
    rust_leb128_tokio_path: Option<PathBuf>,
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
        // wRPC's Rust generator implements none of upstream's codegen variant
        // flags (`--ownership`, `--async`, `--std-feature`,
        // `--merge-structurally-equal-types`, `--map-type`), so every variant
        // invocation (i.e. anything with extra `args`) is expected to fail.
        if !args.is_empty() {
            return true;
        }
        // The wRPC Rust generator does not yet support bare `stream`/`future`
        // types, named fixed-length lists, or `map`. Other async constructs
        // (`error-context`, futures/streams behind resources) generate
        // successfully.
        matches!(
            name,
            "streams.wit" | "futures.wit" | "named-fixed-length-list.wit" | "map.wit"
        )
    }

    fn codegen_test_variants(&self) -> &[(&str, &[&str])] {
        // Mirrors upstream wit-bindgen's codegen variant matrix. wRPC does not
        // implement any of these flags yet, so all variants are marked as
        // expected failures in `should_fail_verify`; they are kept to track
        // parity with upstream and to surface the day a flag becomes supported.
        &[
            ("borrowed", &["--ownership=borrowing"]),
            (
                "borrowed-duplicate",
                &["--ownership=borrowing-duplicate-if-necessary"],
            ),
            ("async", &["--async=all"]),
            ("no-std", &["--std-feature"]),
            ("merge-equal", &["--merge-structurally-equal-types"]),
            ("hashmap", &["--map-type=std::collections::HashMap"]),
        ]
    }

    fn default_bindgen_args(&self) -> &[&str] {
        &["--generate-all"]
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

        let patch = match &opts.rust_leb128_tokio_path {
            Some(path) => format!(
                "\n[patch.crates-io]\nleb128-tokio = {{ path = {:?} }}\n",
                cwd.join(path)
            ),
            None => String::new(),
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
futures = "0.3"
anyhow = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
{patch}"#,
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

/// The fixed driver linking a `runner` (client) and `test` (server) into one
/// binary, connected over an in-process TCP transport.
const DRIVER: &str = r##"#![allow(warnings)]

pub mod test {
    include!(env!("WRPC_TEST_BINDINGS"));
    include!(env!("WRPC_TEST_SRC"));
}

pub mod runner {
    include!(env!("WRPC_RUNNER_BINDINGS"));
    include!(env!("WRPC_RUNNER_SRC"));
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    use core::net::Ipv6Addr;
    use core::time::Duration;
    use std::sync::Arc;

    use futures::{stream, StreamExt as _};
    use wrpc_transport::frame::{tcp, Server};

    let lis = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .expect("failed to start TCP listener");
    let addr = lis.local_addr().expect("failed to get server address");

    let srv = Arc::new(Server::default());

    // Register the `test` world's exported handlers, then serve them forever in
    // the background.
    let invocations = test::serve(srv.as_ref(), test::Component)
        .await
        .expect("failed to serve `test` world exports");
    tokio::spawn(async move {
        let mut invocations = stream::select_all(invocations.into_iter().map(
            |(instance, name, invocations)| invocations.map(move |res| (instance, name, res)),
        ));
        while let Some((instance, name, invocation)) = invocations.next().await {
            invocation
                .unwrap_or_else(|err| panic!("failed to accept `{instance}#{name}` invocation: {err:?}"))
                .await
                .expect("failed to serve invocation");
        }
    });

    // Accept connections forever in the background. Each client invocation opens
    // a new connection.
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move {
            loop {
                let (stream, _) = lis.accept().await.expect("failed to accept connection");
                let (rx, tx) = stream.into_split();
                let srv = Arc::clone(&srv);
                tokio::spawn(async move {
                    srv.accept((), tx, rx)
                        .await
                        .expect("failed to accept connection");
                });
            }
        });
    }

    // Run the `runner` world (client) to completion.
    let clt = tcp::Client::from(addr);
    tokio::time::timeout(Duration::from_secs(60), runner::run(&clt))
        .await
        .expect("runtime test timed out")?;
    Ok(())
}
"##;

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
    /// `test` into one binary connected over an in-process TCP transport.
    pub(crate) fn rust_runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        runner_bindings: &std::path::Path,
        tst: &Component,
        test_bindings: &std::path::Path,
    ) -> Result<()> {
        let artifacts_dir = env::current_dir()?
            .join(&self.opts.artifacts)
            .join(&case.name)
            .join(format!("{}-{}", runner.name, tst.name));

        let runner_rs =
            runner_bindings.join(format!("{}.rs", runner.bindgen.world.to_snake_case()));
        let test_rs = test_bindings.join(format!("{}.rs", tst.bindgen.world.to_snake_case()));

        let driver = artifacts_dir.join("driver.rs");
        super::write_if_different(&driver, DRIVER)?;

        let bin = artifacts_dir.join("driver");
        let mut cmd = self.rustc(Edition::E2021);
        cmd.arg(&driver)
            .arg("--crate-type=bin")
            .arg("-o")
            .arg(&bin)
            .env("WRPC_TEST_BINDINGS", &test_rs)
            .env("WRPC_RUNNER_BINDINGS", &runner_rs)
            .env("WRPC_TEST_SRC", tst.path.canonicalize()?)
            .env("WRPC_RUNNER_SRC", runner.path.canonicalize()?)
            // Set so a `wit_bindgen_wrpc::generate!` embedded in a runner/test
            // source (e.g. the `with` tests' `mod other`) doesn't panic looking
            // for the manifest dir.
            .env("CARGO_MANIFEST_DIR", &artifacts_dir);
        for extern_ in [
            "wit_bindgen_wrpc",
            "wrpc_transport",
            "tokio",
            "futures",
            "anyhow",
            "serde",
            "serde_json",
        ] {
            self.extern_arg(&mut cmd, extern_)?;
        }
        self.run_command(&mut cmd)
            .context("failed to compile runtime test driver")?;

        self.run_command(&mut Command::new(&bin))
            .context("runtime test driver failed")
    }
}
