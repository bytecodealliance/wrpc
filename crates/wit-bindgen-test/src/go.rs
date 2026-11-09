use crate::{Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{Context, Result};
use clap::Parser;
use heck::ToSnakeCase;
use std::env;
use std::path::{Path, PathBuf};
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
        // The wRPC Go generator does not yet support bare `stream` types,
        // `error-context`, fixed-length lists, or `map`.
        matches!(
            name,
            "streams.wit"
                | "error-context.wit"
                | "named-fixed-length-list.wit"
                | "fixed-length-list-in-list.wit"
                | "map.wit"
        )
    }

    fn should_fail_runtime(&self, name: &str) -> bool {
        matches!(
            name,
            // The wRPC Go generator does not yet support fixed-length lists.
            "fixed-length-lists"
                // The wRPC Go generator does not yet support `map`.
                | "map"
                // The wRPC Go generator does not yet support bare `stream` types.
                | "simple-stream"
                // The wRPC Go generator has no `--generate-unused-types` option.
                | "unused-types"
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

/// The driver linking a `runner` (client) and its dependencies into one Go
/// program. Mirrors the Rust driver: each dependency is served over its own
/// in-process TCP transport and its handler receives an invoker routing its
/// imports to the following dependencies (if its `NewHandler` takes one); the
/// `runner` world's `Run` is then called with an invoker routing to all of them.
/// `NewHandler` returns either one handler for all exported interfaces or one
/// per interface, in the order of the world's `Serve` parameters.
const DRIVER: &str = r#"package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"reflect"
	"time"

	wrpc "wrpc.io/go"
	wrpctcp "wrpc.io/go/x/tcp"

	"driver/runner"
{imports}
)

type route struct {
	names  map[[2]string]struct{}
	client wrpc.Invoker
}

type router []route

func (r router) Invoke(ctx context.Context, instance string, name string, paths ...wrpc.SubscribePath) (wrpc.IndexWriteCloser, wrpc.IndexReadCloser, error) {
	for _, rt := range r {
		if _, ok := rt.names[[2]string{instance, name}]; ok {
			return rt.client.Invoke(ctx, instance, name, paths...)
		}
	}
	return nil, nil, fmt.Errorf("no component exports `%s#%s`", instance, name)
}

type recorder struct {
	wrpc.Server
	names map[[2]string]struct{}
}

func (r recorder) Serve(instance string, name string, f wrpc.HandleFunc, paths ...wrpc.SubscribePath) (func() error, error) {
	r.names[[2]string{instance, name}] = struct{}{}
	return r.Server.Serve(instance, name, f, paths...)
}

func serve(ctx context.Context, routes router, serveFn any, newHandler any) router {
	l, err := net.ListenTCP("tcp", &net.TCPAddr{IP: net.IPv6loopback})
	if err != nil {
		panic(err)
	}
	srv := wrpctcp.NewServerWithContext(ctx, l)
	rec := recorder{srv, map[[2]string]struct{}{}}
	h := reflect.ValueOf(newHandler)
	var hArgs []reflect.Value
	if h.Type().NumIn() == 1 {
		hArgs = append(hArgs, reflect.ValueOf(routes))
	}
	handlers := h.Call(hArgs)
	f := reflect.ValueOf(serveFn)
	args := []reflect.Value{reflect.ValueOf(rec)}
	n := f.Type().NumIn() - 1
	if n > 0 && len(handlers) == 0 {
		panic("NewHandler returned no handlers")
	}
	for i := range n {
		args = append(args, handlers[min(i, len(handlers)-1)])
	}
	if err, _ := f.Call(args)[1].Interface().(error); err != nil {
		panic(err)
	}
	go func() {
		for {
			accept, err := srv.Accept()
			if err != nil {
				if !errors.Is(err, net.ErrClosed) {
					os.Stderr.WriteString(err.Error() + "\n")
				}
				return
			}
			go func() {
				if err := accept(ctx); err != nil {
					os.Stderr.WriteString(err.Error() + "\n")
				}
			}()
		}
	}()
	return append(router{{rec.names, wrpctcp.NewInvoker(l.Addr().String())}}, routes...)
}

func main() {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	var routes router
	{serve}

	done := make(chan error, 1)
	go func() {
		done <- runner.Run(ctx, routes)
	}()
	select {
	case err := <-done:
		if err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
	case <-time.After(10 * time.Second):
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
            .arg("--gofmt=false")
            .args(&component.bindgen.args);
        self.run_command(&mut cmd)
            .context("failed to generate Go bindings")?;

        let src = std::fs::read_to_string(&component.path)?;
        // Avoid a `*_test.go` filename (e.g. for a component named `test`); Go
        // excludes those from non-test builds.
        super::write_if_different(&dir.join(format!("{}_impl.go", component.name)), src)?;
        Ok(())
    }

    /// Builds and runs a single Go runtime test by linking the `runner` and
    /// its dependencies into one Go program connected over in-process TCP
    /// transports.
    pub(crate) fn go_runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        deps: &[&(&Component, PathBuf)],
    ) -> Result<()> {
        let cwd = env::current_dir()?;
        let go_module = match &self.opts.go.go_wrpc_path {
            Some(path) => cwd.join(path),
            None => cwd.join("go"),
        };

        let mut name = runner.name.clone();
        for (dep, ..) in deps {
            name.push('-');
            name.push_str(&dep.name);
        }
        let driver = cwd
            .join(&self.opts.artifacts)
            .join(&case.name)
            .join(format!("{name}-go"));

        self.generate_go_component(runner, &driver.join("runner"), "driver/runner")?;
        let mut imports = String::default();
        let mut serve = Vec::default();
        for (dep, ..) in deps.iter().rev() {
            let pkg = dep.bindgen.world.to_snake_case();
            self.generate_go_component(dep, &driver.join(&pkg), &format!("driver/{pkg}"))?;
            imports.push_str(&format!("\t\"driver/{pkg}\"\n"));
            serve.push(format!(
                "routes = serve(ctx, routes, {pkg}.Serve, {pkg}.NewHandler)"
            ));
        }

        super::write_if_different(
            &driver.join("go.mod"),
            format!(
                "module driver\n\ngo 1.25.0\n\nrequire wrpc.io/go v0.0.0-unpublished\n\nreplace wrpc.io/go v0.0.0-unpublished => {}\n",
                go_module.display(),
            ),
        )?;
        super::write_if_different(
            &driver.join("main.go"),
            DRIVER
                .replace("{imports}", imports.trim_end())
                .replace("{serve}", &serve.join("\n\t")),
        )?;

        // The driver module lives under the build artifacts directory, which is
        // nested inside the repository's `go.work`. Disable workspace mode so
        // `go run` uses the driver's own `go.mod` (and its `wrpc.io/go` replace)
        // rather than refusing to build a module absent from `go.work`.
        self.run_command(
            Command::new("go")
                .args(["run", "."])
                .env("GOWORK", "off")
                .current_dir(&driver),
        )
        .context("Go runtime test driver failed")
    }
}
