use crate::{write_if_different, LanguageMethods, Runner, Verify};
use anyhow::{Context, Result};
use clap::Parser;
use std::env;
use std::path::PathBuf;
use std::process::Command;

#[derive(Default, Debug, Clone, Parser)]
pub struct GoOpts {
    /// Path to the in-tree `wrpc.io/go` module that generated Go bindings
    /// depend on. Defaults to `go` relative to the current directory.
    #[clap(long, value_name = "PATH")]
    go_wrpc_path: Option<PathBuf>,
}

pub struct Go;

impl LanguageMethods for Go {
    fn display(&self) -> &str {
        "go"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        // Go runtime tests are not yet supported; `test.go` config is unused.
        Some("//@")
    }

    fn default_bindgen_args(&self) -> &[&str] {
        &["--generate-all", "--package", "bindings", "--gofmt=false"]
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

        write_if_different(&verify.bindings_dir.join("go.work"), "go 1.22.2\nuse .\n")?;
        write_if_different(
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
