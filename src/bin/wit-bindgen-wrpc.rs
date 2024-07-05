use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::str;
use wit_bindgen_core::{wit_parser, Files, WorldGenerator};
use wit_parser::Resolve;

/// Helper for passing VERSION to opt.
/// If `CARGO_VERSION_INFO` is set, use it, otherwise use `CARGO_PKG_VERSION`.
fn version() -> &'static str {
    option_env!("CARGO_VERSION_INFO").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Parser)]
#[command(version = version())]
#[allow(clippy::large_enum_variant)]
enum Opt {
    /// Generates bindings for Rust wRPC applications
    Rust {
        #[clap(flatten)]
        opts: wit_bindgen_wrpc_rust::Opts,
        #[clap(flatten)]
        args: Common,
    },
    /// Generates bindings for Go wRPC applications
    Go {
        #[clap(flatten)]
        opts: wit_bindgen_wrpc_go::Opts,
        #[clap(flatten)]
        args: Common,
    },
}

#[derive(Debug, Parser)]
struct Common {
    /// Where to place output files
    #[clap(long = "out-dir")]
    out_dir: Option<PathBuf>,

    /// Location of WIT file(s) to generate bindings for.
    ///
    /// This path can be either a directory containing `*.wit` files, a `*.wit`
    /// file itself, or a `*.wasm` file which is a wasm-encoded WIT package.
    /// Most of the time it's likely to be a directory containing `*.wit` files
    /// with an optional `deps` folder inside of it.
    #[clap(value_name = "WIT", index = 1)]
    wit: PathBuf,

    /// Optionally specified world that bindings are generated for.
    ///
    /// Bindings are always generated for a world but this option can be omitted
    /// when the WIT package pointed to by the `WIT` option only has a single
    /// world. If there's more than one world in the package then this option
    /// must be specified to name the world that bindings are generated for.
    /// This option can also use the fully qualified syntax such as
    /// `wasi:http/proxy` to select a world from a dependency of the main WIT
    /// package.
    #[clap(short, long)]
    world: Option<String>,

    /// Indicates that no files are written and instead files are checked if
    /// they're up-to-date with the source files.
    #[clap(long)]
    check: bool,

    /// Comma-separated list of features that should be enabled when processing
    /// WIT files.
    ///
    /// This enables using `@unstable` annotations in WIT files.
    #[clap(long)]
    features: Vec<String>,
}

fn main() -> Result<()> {
    let mut files = Files::default();
    let (generator, opt) = match Opt::parse() {
        Opt::Rust { opts, args } => (opts.build(), args),
        Opt::Go { opts, args } => (opts.build(), args),
    };

    gen_world(generator, &opt, &mut files)?;

    for (name, contents) in files.iter() {
        let dst = match &opt.out_dir {
            Some(path) => path.join(name),
            None => name.into(),
        };
        println!("Generating {dst:?}");

        if opt.check {
            let prev = std::fs::read(&dst).with_context(|| format!("failed to read {dst:?}"))?;
            if prev != contents {
                // The contents differ. If it looks like textual contents, do a
                // line-by-line comparison so that we can tell users what the
                // problem is directly.
                if let (Ok(utf8_prev), Ok(utf8_contents)) =
                    (str::from_utf8(&prev), str::from_utf8(contents))
                {
                    if !utf8_prev
                        .chars()
                        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                        && utf8_prev.lines().eq(utf8_contents.lines())
                    {
                        bail!("{} differs only in line endings (CRLF vs. LF). If this is a text file, configure git to mark the file as `text eol=lf`.", dst.display());
                    }
                }
                // The contents are binary or there are other differences; just
                // issue a generic error.
                bail!("not up to date: {}", dst.display());
            }
            continue;
        }

        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {parent:?}"))?;
        }
        std::fs::write(&dst, contents).with_context(|| format!("failed to write {dst:?}"))?;
    }

    Ok(())
}

fn gen_world(
    mut generator: Box<dyn WorldGenerator>,
    opts: &Common,
    files: &mut Files,
) -> Result<()> {
    let mut resolve = Resolve::default();
    for features in &opts.features {
        for feature in features
            .split(',')
            .flat_map(|s| s.split_whitespace())
            .filter(|f| !f.is_empty())
        {
            resolve.features.insert(feature.to_string());
        }
    }
    let (pkgs, _files) = resolve.push_path(&opts.wit)?;
    let world = resolve.select_world(&pkgs, opts.world.as_deref())?;
    generator.generate(&resolve, world, files)?;

    Ok(())
}

#[test]
fn verify_cli() {
    use clap::CommandFactory;
    Opt::command().debug_assert();
}
