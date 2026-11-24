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
        _name: &str,
        config: &crate::config::WitConfig,
        _args: &[String],
    ) -> bool {
        // wRPC does not support `error-context`.
        config.error_context
    }

    fn should_fail_runtime(&self, name: &str) -> bool {
        // The wRPC Go generator has no `--generate-unused-types` option.
        name == "unused-types"
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
/// program. Mirrors the Rust driver: each in-process TCP listener serves the
/// chain of dependencies following the previous one, so that invocations are
/// served by the nearest following dependency exporting them, and each
/// dependency's handler connects to the listener serving the chain following it
/// (if its `NewHandler` takes a `connect` function). The `runner` world's `Run`
/// connects to the listener serving all of them. `NewHandler` returns either one
/// handler for all exported interfaces or one per interface, in the order of
/// the world's `Handle` parameters.
const DRIVER: &str = r##"package main

import (
	"context"
	"io"
	"net"
	"os"
	"reflect"
	"time"

	wrpc "wrpc.io/go"

	"driver/runner"
{imports}
)

type handle func(context.Context, wrpc.Header, *wrpc.Framed) (bool, error)

func connector(l net.Listener) func() (*wrpc.Framed, error) {
	return func() (*wrpc.Framed, error) {
		conn, err := net.Dial("tcp", l.Addr().String())
		if err != nil {
			return nil, err
		}
		return wrpc.NewFramed(conn, conn), nil
	}
}

func handler(handleFn any, newHandler any, connect func() (*wrpc.Framed, error)) handle {
	h := reflect.ValueOf(newHandler)
	var hArgs []reflect.Value
	if h.Type().NumIn() == 1 {
		hArgs = append(hArgs, reflect.ValueOf(connect))
	}
	handlers := h.Call(hArgs)
	f := reflect.ValueOf(handleFn)
	n := f.Type().NumIn() - 3
	if n > 0 && len(handlers) == 0 {
		panic("NewHandler returned no handlers")
	}
	return func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error) {
		args := []reflect.Value{reflect.ValueOf(ctx), reflect.ValueOf(hdr), reflect.ValueOf(c)}
		for i := range n {
			args = append(args, handlers[min(i, len(handlers)-1)])
		}
		out := f.Call(args)
		err, _ := out[1].Interface().(error)
		return out[0].Bool(), err
	}
}

func serve(ctx context.Context, l net.Listener, chain []handle) {
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				lis := wrpc.NewListener(conn, conn)
				for {
					hdr, c, err := lis.Accept(ctx)
					if err == io.EOF {
						return
					}
					if err != nil {
						os.Stderr.WriteString("failed to read invocation header: " + err.Error() + "\n")
						return
					}
					var ok bool
					for _, h := range chain {
						if ok, err = h(ctx, hdr, c); ok {
							break
						}
					}
					if !ok {
						os.Stderr.WriteString("no component exports `" + hdr.Instance + "#" + hdr.Name + "`\n")
						return
					}
					if err != nil {
						os.Stderr.WriteString("failed to serve invocation: " + err.Error() + "\n")
						return
					}
				}
			}()
		}
	}()
}

func main() {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	var ls []net.Listener
	for range {listeners} {
		l, err := net.Listen("tcp", "[::1]:0")
		if err != nil {
			panic(err)
		}
		defer l.Close()
		ls = append(ls, l)
	}
	var handlers []handle
	{handlers}
	for i, l := range ls {
		serve(ctx, l, handlers[i:])
	}

	done := make(chan error, 1)
	go func() {
		done <- runner.Run(ctx, connector(ls[0]))
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
"##;

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
        let mut handlers = Vec::default();
        for (i, (dep, ..)) in deps.iter().enumerate() {
            let pkg = dep.bindgen.world.to_snake_case();
            self.generate_go_component(dep, &driver.join(&pkg), &format!("driver/{pkg}"))?;
            imports.push_str(&format!("\t\"driver/{pkg}\"\n"));
            handlers.push(format!(
                "handlers = append(handlers, handler({pkg}.Handle, {pkg}.NewHandler, connector(ls[{}])))",
                i + 1
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
                .replace("{listeners}", &(deps.len() + 1).to_string())
                .replace("{handlers}", &handlers.join("\n\t")),
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
