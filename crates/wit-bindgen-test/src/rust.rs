use crate::{write_if_different, Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{bail, Context, Result};
use clap::Parser;
use heck::ToSnakeCase;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::Command;

#[derive(Default, Debug, Clone, Parser)]
pub struct RustOpts {
    /// Path to the in-tree `wit-bindgen-wrpc` runtime crate that generated Rust
    /// bindings depend on.
    #[clap(long, value_name = "PATH")]
    rust_wit_bindgen_path: Option<PathBuf>,

    /// Path to the in-tree `wrpc-transport` crate used by the runtime-test
    /// driver. Defaults to the `transport` crate adjacent to
    /// `--rust-wit-bindgen-path`.
    #[clap(long, value_name = "PATH")]
    rust_wrpc_transport_path: Option<PathBuf>,
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

    fn default_bindgen_args(&self) -> &[&str] {
        &["--generate-all"]
    }

    fn prepare(&self, runner: &mut Runner<'_>) -> Result<()> {
        let cwd = env::current_dir()?;
        let opts = &runner.opts.rust;

        let wit_bindgen = match &opts.rust_wit_bindgen_path {
            Some(path) => cwd.join(path),
            None => cwd.join("crates/wit-bindgen"),
        };
        let transport = match &opts.rust_wrpc_transport_path {
            Some(path) => cwd.join(path),
            None => wit_bindgen
                .parent()
                .context("`--rust-wit-bindgen-path` has no parent directory")?
                .join("transport"),
        };

        let dir = cwd.join(&runner.opts.artifacts).join("rust");
        let helper = dir.join("deps-crate");

        write_if_different(
            &helper.join("Cargo.toml"),
            format!(
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
wit-bindgen-wrpc = {{ path = {wit_bindgen:?} }}
wrpc-transport = {{ path = {transport:?}, features = ["net"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread", "net", "sync", "io-util", "time"] }}
futures = "0.3"
anyhow = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#,
            ),
        )?;
        write_if_different(&helper.join("lib.rs"), "")?;

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
        // The generator writes a single `<world>.rs`; find it without needing to
        // know the (auto-selected) world name.
        let bindings = std::fs::read_dir(verify.bindings_dir)
            .with_context(|| format!("failed to read {:?}", verify.bindings_dir))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|s| s.to_str()) == Some("rs"))
            .context("no generated Rust bindings were produced")?;
        let mut cmd = runner.rustc();
        cmd.arg(&bindings);
        runner.extern_arg(&mut cmd, "wit_bindgen_wrpc")?;
        cmd.arg("--crate-type=rlib")
            .arg("-Dwarnings")
            .arg("-o")
            .arg(verify.artifacts_dir.join("libtmp.rlib"));
        runner.run_command(&mut cmd)
    }
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
    fn rustc(&self) -> Command {
        let state = self.rust_state.as_ref().unwrap();
        let mut cmd = Command::new("rustc");
        cmd.arg("--edition=2021")
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
        test: &Test,
        runner: &Component,
        tst: &Component,
    ) -> Result<()> {
        let artifacts_dir = env::current_dir()?
            .join(&self.opts.artifacts)
            .join("runtime")
            .join(&test.name)
            .join(format!("{}-{}", runner.name, tst.name));
        let bindings_dir = artifacts_dir.join("bindings");

        Rust.generate_bindings(self, &runner.bindgen, &bindings_dir)
            .context("failed to generate `runner` bindings")?;
        Rust.generate_bindings(self, &tst.bindgen, &bindings_dir)
            .context("failed to generate `test` bindings")?;

        let runner_bindings =
            bindings_dir.join(format!("{}.rs", runner.bindgen.world.to_snake_case()));
        let test_bindings = bindings_dir.join(format!("{}.rs", tst.bindgen.world.to_snake_case()));

        let driver = artifacts_dir.join("driver.rs");
        write_if_different(&driver, DRIVER)?;

        let bin = artifacts_dir.join("driver");
        let mut cmd = self.rustc();
        cmd.arg(&driver)
            .arg("--crate-type=bin")
            .arg("-Dwarnings")
            .arg("-o")
            .arg(&bin)
            .env("WRPC_TEST_BINDINGS", &test_bindings)
            .env("WRPC_RUNNER_BINDINGS", &runner_bindings)
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
