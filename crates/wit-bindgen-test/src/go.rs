use crate::{Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{Context, Result};
use clap::Parser;
use std::env;
use std::path::Path;
use std::process::Command;

#[derive(Default, Debug, Clone, Parser)]
pub struct GoOpts {
    /// Path to the in-tree `wrpc.io/go` module that generated Go bindings
    /// depend on. Defaults to `go` relative to the current directory.
    #[clap(long, value_name = "PATH")]
    go_wrpc_path: Option<std::path::PathBuf>,
}

pub struct Go;

impl LanguageMethods for Go {
    fn display(&self) -> &str {
        "go"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
    }

    fn default_bindgen_args(&self) -> &[&str] {
        &["--generate-all", "--package", "bindings", "--gofmt=false"]
    }

    fn should_fail_verify(
        &self,
        name: &str,
        _config: &crate::config::WitConfig,
        _args: &[String],
    ) -> bool {
        // The wRPC Go generator does not yet support bare `stream`/`future`
        // types, `error-context`, or futures held by resources.
        matches!(
            name,
            "streams.wit"
                | "futures.wit"
                | "error-context.wit"
                | "resources-with-futures.wit"
                | "future-same-type-different-names.wit"
                | "named-fixed-length-list.wit"
                | "map.wit"
        )
    }

    fn prepare(&self, _runner: &mut Runner<'_>) -> Result<()> {
        // Go modules are resolved at build time; nothing to prepare.
        Ok(())
    }

    fn verify(&self, runner: &Runner<'_>, verify: &Verify<'_>) -> Result<()> {
        let cwd = env::current_dir()?;
        let go_module = match &runner.opts.go.go_wrpc_path {
            Some(path) => cwd.join(path),
            None => cwd.join("go"),
        };

        crate::write_if_different(&verify.bindings_dir.join("go.work"), "go 1.22.2\nuse .\n")?;
        crate::write_if_different(
            &verify.bindings_dir.join("go.mod"),
            format!(
                "module bindings\n\ngo 1.22.2\n\nrequire wrpc.io/go v0.0.0-unpublished\n\nreplace wrpc.io/go v0.0.0-unpublished => {}\n",
                go_module.display(),
            ),
        )?;

        runner
            .run_command(
                Command::new("go")
                    .args(["build", "./..."])
                    .current_dir(verify.bindings_dir),
            )
            .context("failed to compile generated Go bindings")
    }
}

/// The fixed driver linking a `runner` (client) and `test` (server) into one
/// Go program, connected over an in-process TCP transport. Mirrors the Rust
/// driver: it serves the `test` world's exports and runs the `runner` world's
/// `Run` against a client connected to the in-process listener.
const DRIVER: &str = r#"package main

import (
	"context"
	"errors"
	"net"
	"os"
	"time"

	wrpctcp "wrpc.io/go/x/tcp"

	"driver/runner"
	"driver/test"
)

func main() {
	a, err := net.ResolveTCPAddr("tcp", "[::1]:0")
	if err != nil {
		panic(err)
	}
	l, err := net.ListenTCP("tcp", a)
	if err != nil {
		panic(err)
	}
	addr := l.Addr().String()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	srv := wrpctcp.NewServerWithContext(ctx, l)
	stop, err := test.Serve(srv, test.NewHandler())
	if err != nil {
		panic(err)
	}

	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			default:
			}
			if err := srv.Accept(); err != nil {
				if errors.Is(err, net.ErrClosed) {
					return
				}
				return
			}
		}
	}()

	client := wrpctcp.NewInvoker(addr)
	done := make(chan error, 1)
	go func() {
		done <- runner.Run(ctx, client)
	}()
	select {
	case err := <-done:
		cancel()
		_ = stop()
		_ = l.Close()
		if err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
	case <-time.After(60 * time.Second):
		panic("runtime test timed out")
	}
}
"#;

impl Runner<'_> {
    /// Generates Go bindings for `component`'s world into `dir` using `pkg` as
    /// the module import path, then copies the component's source file in
    /// beside them so both share the generated world package.
    fn generate_go_component(&self, component: &Component, dir: &Path, pkg: &str) -> Result<()> {
        let mut cmd = Command::new(self.wit_bindgen);
        cmd.arg("go")
            .arg(&component.bindgen.wit_path)
            .arg("--world")
            .arg(format!("%{}", component.bindgen.world))
            .arg("--out-dir")
            .arg(dir)
            .arg("--generate-all")
            .arg("--package")
            .arg(pkg)
            .arg("--gofmt=false");
        self.run_command(&mut cmd)
            .context("failed to generate Go bindings")?;

        let src = std::fs::read_to_string(&component.path)?;
        // Avoid a `*_test.go` filename (e.g. for a component named `test`); Go
        // excludes those from non-test builds.
        super::write_if_different(&dir.join(format!("{}_impl.go", component.name)), src)?;
        Ok(())
    }

    /// Builds and runs a single Go runtime test by linking the `runner` and
    /// `test` into one Go program connected over an in-process TCP transport.
    pub(crate) fn go_runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        tst: &Component,
    ) -> Result<()> {
        let cwd = env::current_dir()?;
        let go_module = match &self.opts.go.go_wrpc_path {
            Some(path) => cwd.join(path),
            None => cwd.join("go"),
        };

        let driver = cwd
            .join(&self.opts.artifacts)
            .join(&case.name)
            .join(format!("{}-{}-go", runner.name, tst.name));

        self.generate_go_component(runner, &driver.join("runner"), "driver/runner")?;
        self.generate_go_component(tst, &driver.join("test"), "driver/test")?;

        super::write_if_different(
            &driver.join("go.mod"),
            format!(
                "module driver\n\ngo 1.25.0\n\nrequire wrpc.io/go v0.0.0-unpublished\n\nreplace wrpc.io/go v0.0.0-unpublished => {}\n",
                go_module.display(),
            ),
        )?;
        super::write_if_different(&driver.join("main.go"), DRIVER)?;

        self.run_command(Command::new("go").args(["run", "."]).current_dir(&driver))
            .context("Go runtime test driver failed")
    }
}
