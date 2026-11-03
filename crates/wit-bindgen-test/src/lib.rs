// This crate is vendored from upstream `wit-bindgen`'s `crates/test` (with the
// guest-Wasm-specific pieces removed); these lints fire on the upstream code,
// which is kept verbatim to minimize the diff.
#![allow(
    clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args,
    clippy::println_empty_string,
    clippy::flat_map_identity,
    clippy::nonminimal_bool
)]

use core::iter;
use core::ptr;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

mod config;
mod go;
mod js;
mod rust;

/// Tool to run tests that exercise the `wit-bindgen` bindings generator.
///
/// This tool is used to (a) generate bindings for a target language, (b)
/// compile the bindings and source code to a wasm component, (c) compose a
/// "runner" and a "test" component together, and (d) execute this component to
/// ensure that it passes. This process is guided by filesystem structure which
/// must adhere to some conventions.
///
/// * Tests are located in any directory that contains a `test.wit` description
///   of the WIT being tested. The `<TEST>` argument to this command is walked
///   recursively to find `test.wit` files.
///
/// * The `test.wit` file must have a `runner` world and a `test` world. The
///   "runner" should import interfaces that are exported by "test". A
///   `//@ dependencies = ['a', 'b']` comment replaces the `test` world with
///   the listed worlds, whose source files are named after them (`a*.*`,
///   `b*.*`). The runner's and each dependency's imports are routed to the
///   nearest following dependency exporting them.
///
/// * Adjacent to `test.wit` should be a number of `runner*.*` files. There is
///   one runner per source language, for example `runner.rs` and `runner.c`.
///   These are source files for the `runner` world. Source files can start with
///   `//@ ...` comments to deserialize into `config::RuntimeTestConfig`,
///   currently that supports:
///
///   ```text
///   //@ args = ['--arguments', 'to', '--the', 'bindings', '--generator']
///   ```
///
///   or
///
///   ```text
///   //@ args = '--arguments to --the bindings --generator'
///   ```
///
/// * Adjacent to `test.wit` should also be a number of `test*.*` files. Like
///   runners there is one per source language. Note that you can have multiple
///   implementations of tests in the same language too, for example
///   `test-foo.rs` and `test-bar.rs`. All tests must export the same `test`
///   world from `test.wit`, however.
///
/// This tool will discover `test.wit` files, discover runners/tests, and then
/// compile everything and run the combinatorial matrix of runners against
/// tests. It's expected that each `runner.*` and `test.*` perform the same
/// functionality and only differ in source language.
#[derive(Default, Debug, Clone, Parser)]
pub struct Opts {
    /// Directory containing the test being run or all tests being run.
    test: Vec<PathBuf>,

    /// Path to where binary artifacts for tests are stored.
    #[clap(long, value_name = "PATH")]
    artifacts: PathBuf,

    /// Optional filter to use on test names to only run some tests.
    ///
    /// This is a regular expression defined by the `regex` Rust crate.
    #[clap(short, long, value_name = "REGEX")]
    filter: Option<regex::Regex>,

    #[clap(flatten)]
    rust: rust::RustOpts,

    #[clap(flatten)]
    go: go::GoOpts,

    #[clap(flatten)]
    js: js::JsOpts,

    /// Whether or not the calling process's stderr is inherited into child
    /// processes.
    ///
    /// This helps preserving color in compiler error messages but can also
    /// jumble up output if there are multiple errors.
    #[clap(short, long)]
    inherit_stderr: bool,

    /// Passing `--lang rust` will only test Rust for example. If not specified
    /// all languages are tested.
    #[clap(short, long, value_delimiter = ',')]
    languages: Vec<String>,
}

impl Opts {
    pub fn run(&self, wit_bindgen: &Path) -> Result<()> {
        Runner {
            opts: self,
            rust_state: None,
            wit_bindgen,
        }
        .run()
    }
}

/// Helper structure representing a discovered `test.wit` file.
struct Test {
    /// The name of this test, unique amongst all tests.
    ///
    /// Inferred from the directory name.
    name: String,

    kind: TestKind,
}

enum TestKind {
    Runtime(Vec<Component>),
    Codegen(PathBuf),
}

/// Helper structure representing a single component found in a test directory.
struct Component {
    /// The name of this component, inferred from the file stem.
    ///
    /// May be shared across different languages.
    name: String,

    /// The path to the source file for this component.
    path: PathBuf,

    /// Whether or not this component is a "runner" or a "test"
    kind: Kind,

    /// The detected language for this component.
    language: Language,

    /// The WIT world that's being used with this component, loaded from
    /// `test.wit`.
    bindgen: Bindgen,
}

#[derive(Clone)]
struct Bindgen {
    /// The arguments to the bindings generator that this component will be
    /// using.
    args: Vec<String>,
    /// The path to the `*.wit` file or files that are having bindings
    /// generated.
    wit_path: PathBuf,
    /// The name of the world within `wit_path` that's having bindings generated
    /// for it.
    world: String,
    /// Configuration found in `wit_path`
    wit_config: config::WitConfig,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Runner,
    Test,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Language {
    Rust,
    Go,
    Js,
}

/// Helper structure to package up arguments when sent to language-specific
/// compilation backends for `LanguageMethods::verify`
struct Verify<'a> {
    // `wit_test` and `args` are carried from upstream but only consulted by the
    // guest-Wasm/no_std backends that wRPC does not vendor.
    #[expect(dead_code)]
    wit_test: &'a Path,
    bindings_dir: &'a Path,
    artifacts_dir: &'a Path,
    #[expect(dead_code)]
    args: &'a [String],
    world: &'a str,
}

/// Helper structure to package up runtime state associated with executing tests.
struct Runner<'a> {
    opts: &'a Opts,
    rust_state: Option<rust::State>,
    wit_bindgen: &'a Path,
}

impl Runner<'_> {
    /// Executes all tests.
    fn run(&mut self) -> Result<()> {
        // First step, discover all tests in the specified test directory.
        let mut tests = HashMap::new();
        for test in self.opts.test.iter() {
            self.discover_tests(&mut tests, test)
                .with_context(|| format!("failed to discover tests in {test:?}"))?;
        }
        if tests.is_empty() {
            bail!(
                "no `test.wit` files found were found in {:?}",
                self.opts.test,
            );
        }

        self.prepare_languages(&tests)?;
        self.run_codegen_tests(&tests)?;
        self.run_runtime_tests(&tests)?;

        println!("PASSED");

        Ok(())
    }

    /// Walks over `dir`, recursively, inserting located cases into `tests`.
    fn discover_tests(&self, tests: &mut HashMap<String, Test>, path: &Path) -> Result<()> {
        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("wit") {
                return self.insert_test(&path, TestKind::Codegen(path.to_owned()), tests);
            }

            return Ok(());
        }

        let runtime_candidate = path.join("test.wit");
        if runtime_candidate.is_file() {
            let components = self
                .load_test(&runtime_candidate, path)
                .with_context(|| format!("failed to load test in {path:?}"))?;
            return self.insert_test(path, TestKind::Runtime(components), tests);
        }

        let codegen_candidate = path.join("wit");
        if codegen_candidate.is_dir() {
            return self.insert_test(path, TestKind::Codegen(codegen_candidate), tests);
        }

        for entry in path.read_dir().context("failed to read test directory")? {
            let entry = entry.context("failed to read test directory entry")?;
            let path = entry.path();

            self.discover_tests(tests, &path)?;
        }

        Ok(())
    }

    fn insert_test(
        &self,
        path: &Path,
        kind: TestKind,
        tests: &mut HashMap<String, Test>,
    ) -> Result<()> {
        let test_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("non-utf-8 filename")?;
        let prev = tests.insert(
            test_name.to_string(),
            Test {
                name: test_name.to_string(),
                kind,
            },
        );
        if prev.is_some() {
            bail!("duplicate test name `{test_name}` found");
        }
        Ok(())
    }

    /// Loads a test from `dir` using the `wit` file in the directory specified.
    ///
    /// Returns a list of components that were found within this directory.
    fn load_test(&self, wit: &Path, dir: &Path) -> Result<Vec<Component>> {
        let mut resolve = wit_parser::Resolve::default();
        // Use the test directory when it has a `deps/` directory of dependency
        // packages, like upstream, so those are resolved too.
        let wit_path = if dir.join("deps").exists() { dir } else { wit };
        let (pkg, _) = resolve
            .push_path(wit_path)
            .context("failed to load `test.wit` in test directory")?;
        let resolve = Arc::new(resolve);

        let wit_contents = std::fs::read_to_string(wit)?;
        let wit_config: config::WitConfig = config::parse_test_config(&wit_contents, "//@")
            .context("failed to parse WIT test config")?;

        let mut worlds = vec![(wit_config.runner_world().to_string(), Kind::Runner)];
        for world in wit_config.dependency_worlds() {
            worlds.push((world, Kind::Test));
        }
        for (world, ..) in &worlds {
            resolve.select_world(&[pkg], Some(world)).with_context(|| {
                format!("failed to find expected `{world}` world to generate bindings")
            })?;
        }

        let mut components = Vec::new();

        for entry in dir.read_dir().context("failed to read test directory")? {
            let entry = entry.context("failed to read test directory entry")?;
            let path = entry.path();

            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name == "test.wit" {
                continue;
            }
            let Some((world, kind)) = worlds
                .iter()
                .filter(|(world, ..)| name.starts_with(world))
                .max_by_key(|(world, ..)| world.len())
            else {
                continue;
            };

            let bindgen = Bindgen {
                args: Vec::new(),
                wit_config: wit_config.clone(),
                world: world.clone(),
                wit_path: wit_path.to_path_buf(),
            };

            let component = self
                .parse_component(&path, *kind, bindgen)
                .with_context(|| format!("failed to parse component source file {path:?}"))?;
            components.push(component);
        }

        for (world, ..) in &worlds {
            if !components.iter().any(|c| c.bindgen.world == *world) {
                bail!("no `{world}*` test files found in test directory");
            }
        }

        Ok(components)
    }

    /// Parsers the component located at `path` and creates all information
    /// necessary for a `Component` return value.
    fn parse_component(&self, path: &Path, kind: Kind, mut bindgen: Bindgen) -> Result<Component> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .context("non-utf-8 path extension")?;

        let language = match extension {
            "rs" => Language::Rust,
            "go" => Language::Go,
            "js" => Language::Js,
            other => bail!("unsupported test source file extension `{other}`"),
        };

        let contents = fs::read_to_string(&path)?;
        let config = match language.obj().comment_prefix_for_test_config() {
            Some(comment) => {
                config::parse_test_config::<config::RuntimeTestConfig>(&contents, comment)?
            }
            None => Default::default(),
        };
        assert!(bindgen.args.is_empty());
        bindgen.args = config.args.into();

        Ok(Component {
            name: path.file_stem().unwrap().to_str().unwrap().to_string(),
            path: path.to_path_buf(),
            language,
            bindgen,
            kind,
        })
    }

    /// Prepares all languages in use in `test` as part of a one-time
    /// initialization step.
    fn prepare_languages(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let all_languages = self.all_languages();

        let mut prepared = HashSet::new();
        let mut prepare = |lang: &Language| -> Result<()> {
            if !self.include_language(lang) || !prepared.insert(lang.clone()) {
                return Ok(());
            }
            lang.obj()
                .prepare(self)
                .with_context(|| format!("failed to prepare language {lang}"))
        };

        for test in tests.values() {
            match &test.kind {
                TestKind::Runtime(c) => {
                    for component in c {
                        prepare(&component.language)?
                    }
                }
                TestKind::Codegen(_) => {
                    for lang in all_languages.iter() {
                        prepare(lang)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn all_languages(&self) -> Vec<Language> {
        Language::ALL.to_vec()
    }

    /// Executes all tests that are `TestKind::Codegen`.
    fn run_codegen_tests(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let mut codegen_tests = Vec::new();
        let languages = self.all_languages();
        for (name, test) in tests.iter().filter_map(|(name, t)| match &t.kind {
            TestKind::Runtime(_) => None,
            TestKind::Codegen(p) => Some((name, p)),
        }) {
            let config = match fs::read_to_string(test) {
                Ok(wit) => config::parse_test_config::<config::WitConfig>(&wit, "//@")
                    .with_context(|| format!("failed to parse test config from {test:?}"))?,
                Err(_) => Default::default(),
            };
            for language in languages.iter() {
                // If the CLI arguments filter out this language, then discard
                // the test case.
                if !self.include_language(&language) {
                    continue;
                }

                codegen_tests.push((
                    language.clone(),
                    test,
                    name.to_string(),
                    Vec::new(),
                    config.clone(),
                ));

                for (args_kind, args) in language.obj().codegen_test_variants() {
                    codegen_tests.push((
                        language.clone(),
                        test,
                        format!("{name}-{args_kind}"),
                        args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        config.clone(),
                    ));
                }
            }
        }

        if codegen_tests.is_empty() {
            return Ok(());
        }

        println!("Running {} codegen tests:", codegen_tests.len());

        let results = codegen_tests
            .par_iter()
            .map(|(language, test, args_kind, args, config)| {
                let should_fail = language.obj().should_fail_verify(args_kind, config, args);
                let result = self
                    .codegen_test(language, test, &args_kind, args, config)
                    .with_context(|| {
                        format!("failed to codegen test for `{language}` over {test:?}")
                    });
                self.update_status(&result, should_fail);
                (result, should_fail, language, test, args_kind)
            })
            .collect::<Vec<_>>();

        println!("");

        self.render_errors(results.into_iter().map(
            |(result, should_fail, language, test, args_kind)| {
                StepResult::new(test.to_str().unwrap(), result)
                    .should_fail(should_fail)
                    .metadata("language", language)
                    .metadata("variant", args_kind)
            },
        ));

        Ok(())
    }

    /// Runs a single codegen test.
    ///
    /// This will generate bindings for `test` in the `language` specified. The
    /// test name is mangled by `args_kind` and the `args` are arguments to pass
    /// to the bindings generator.
    fn codegen_test(
        &self,
        language: &Language,
        test: &Path,
        args_kind: &str,
        args: &[String],
        config: &config::WitConfig,
    ) -> Result<()> {
        let mut resolve = wit_parser::Resolve::default();
        let (pkg, _) = resolve.push_path(test).context("failed to load WIT")?;
        let world = resolve
            .select_world(&[pkg], None)
            .or_else(|err| {
                resolve
                    .select_world(&[pkg], Some("imports"))
                    .map_err(|_| err)
            })
            .context("failed to select a world for bindings generation")?;
        let world = resolve.worlds[world].name.clone();

        let artifacts_dir = std::env::current_dir()?
            .join(&self.opts.artifacts)
            .join("codegen")
            .join(language.to_string())
            .join(args_kind);
        let bindings_dir = artifacts_dir.join("bindings");
        let bindgen = Bindgen {
            args: args.to_vec(),
            wit_path: test.to_path_buf(),
            world: world.clone(),
            wit_config: config.clone(),
        };
        language
            .obj()
            .generate_bindings(self, &bindgen, &bindings_dir)
            .context("failed to generate bindings")?;

        language
            .obj()
            .verify(
                self,
                &Verify {
                    world: &world,
                    artifacts_dir: &artifacts_dir,
                    bindings_dir: &bindings_dir,
                    wit_test: test,
                    args: &bindgen.args,
                },
            )
            .context("failed to verify generated bindings")?;

        Ok(())
    }

    /// Execute all `TestKind::Runtime` tests
    fn run_runtime_tests(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let components = tests
            .values()
            .filter(|t| match &self.opts.filter {
                Some(filter) => filter.is_match(&t.name),
                None => true,
            })
            .filter_map(|t| match &t.kind {
                TestKind::Runtime(c) => Some(c.iter().map(move |c| (t, c))),
                TestKind::Codegen(_) => None,
            })
            .flat_map(|i| i)
            // Discard components that are unrelated to the languages being
            // tested.
            .filter(|(_test, component)| self.include_language(&component.language))
            .collect::<Vec<_>>();

        println!("Compiling {} components:", components.len());

        // In parallel compile all sources to their binary component
        // form.
        let compile_results = components
            .par_iter()
            .map(|(test, component)| {
                let should_fail = component.language.obj().should_fail_runtime(&test.name);
                let path = self
                    .compile_component(test, component)
                    .with_context(|| format!("failed to compile component {:?}", component.path));
                self.update_status(&path, should_fail && path.is_err());
                (test, component, path, should_fail)
            })
            .collect::<Vec<_>>();
        println!("");

        let mut compilations = Vec::new();
        let mut expected = Vec::default();
        let mut failed = Vec::default();
        self.render_errors(compile_results.into_iter().map(
            |(test, component, result, should_fail)| match result {
                Ok(path) => {
                    if should_fail {
                        expected.push((*test, *component));
                    }
                    compilations.push((test, component, path));
                    StepResult::new("", Ok(()))
                }
                Err(e) => {
                    failed.push((test.name.as_str(), &component.language));
                    StepResult::new(&test.name, Err(e))
                        .should_fail(should_fail)
                        .metadata("component", &component.name)
                        .metadata("path", component.path.display())
                }
            },
        ));

        // Next, massage the data a bit. Create a map of all tests to where
        // their components are located. Then perform a product of runners/tests
        // to generate a list of test cases. Finally actually execute the test
        // cases.
        let mut compiled_components = HashMap::new();
        for (test, component, path) in compilations {
            let list = compiled_components.entry(&test.name).or_insert(Vec::new());
            list.push((*component, path));
        }

        let mut to_run = Vec::new();
        for (test, components) in compiled_components.iter() {
            let TestKind::Runtime(all) = &tests[test.as_str()].kind else {
                unreachable!()
            };
            let worlds = all
                .iter()
                .find(|c| c.kind == Kind::Runner)
                .map(|c| c.bindgen.wit_config.dependency_worlds())
                .unwrap_or_default();
            for a in components.iter().filter(|(c, _)| c.kind == Kind::Runner) {
                // Unlike upstream's component composition, wRPC links the runner
                // and its dependencies into one host binary, so they must share
                // a language.
                let mut deps = vec![Vec::default()];
                for world in &worlds {
                    deps = deps
                        .into_iter()
                        .flat_map(|deps| {
                            components
                                .iter()
                                .filter(|(c, ..)| {
                                    c.kind == Kind::Test
                                        && c.bindgen.world == *world
                                        && c.language == a.0.language
                                })
                                .map(move |b| {
                                    let mut deps = deps.clone();
                                    deps.push(b);
                                    deps
                                })
                        })
                        .collect();
                }
                for deps in deps {
                    to_run.push((test, a, deps));
                }
            }
        }

        println!("Running {} runtime tests:", to_run.len());

        let results = to_run
            .par_iter()
            .map(|(case_name, (runner, runner_path), deps)| {
                let case = &tests[case_name.as_str()];
                let result = self
                    .runtime_test(case, runner, runner_path, deps)
                    .with_context(|| {
                        format!(
                            "failed to run `{}` with runner `{}`",
                            case.name,
                            runner.path.display(),
                        )
                    });
                let should_fail = runner.language.obj().should_fail_runtime(&case.name);
                self.update_status(&result, should_fail);
                (result, should_fail, case_name, runner, runner_path, deps)
            })
            .collect::<Vec<_>>();

        println!("");

        let ran: Vec<&Component> = to_run
            .iter()
            .flat_map(|(.., runner, deps)| {
                iter::once(*runner)
                    .chain(deps.iter().copied())
                    .map(|(component, ..)| *component)
            })
            .collect();
        let failed: Vec<(&str, &Language)> = results
            .iter()
            .filter(|(result, ..)| result.is_err())
            .map(|(.., case_name, runner, _, _)| (case_name.as_str(), &runner.language))
            .chain(failed)
            .collect();
        let stale = expected
            .into_iter()
            .filter(|(test, component)| {
                !ran.iter().any(|ran| ptr::eq(*ran, *component))
                    && !failed.iter().any(|(name, language)| {
                        *name == test.name && **language == component.language
                    })
                    && components.iter().any(|(t, c)| {
                        t.name == test.name
                            && c.language == component.language
                            && c.kind == Kind::Runner
                    })
            })
            .map(|(test, component)| {
                StepResult::new(&test.name, Ok(()))
                    .should_fail(true)
                    .metadata("component", &component.name)
                    .metadata("path", component.path.display())
            });

        self.render_errors(
            results
                .into_iter()
                .map(
                    |(result, should_fail, case_name, runner, runner_path, deps)| {
                        let mut step = StepResult::new(case_name, result)
                            .should_fail(should_fail)
                            .metadata("runner", runner.path.display())
                            .metadata("compiled runner", runner_path.display());
                        for (test, test_path) in deps {
                            step = step
                                .metadata("test", test.path.display())
                                .metadata("compiled test", test_path.display());
                        }
                        step
                    },
                )
                .chain(stale),
        );

        Ok(())
    }

    /// Generates bindings for the `component` specified in the `test` given.
    ///
    /// Unlike upstream `wit-bindgen` this does not compile a standalone guest
    /// component — wRPC bindings are `Invoke`/`Serve` stubs that are linked into
    /// a host binary per `runner`/`test` pair in `runtime_test`. Returns the
    /// directory the bindings were generated into.
    fn compile_component(&self, test: &Test, component: &Component) -> Result<PathBuf> {
        let root_dir = std::env::current_dir()?
            .join(&self.opts.artifacts)
            .join(&test.name);
        let artifacts_dir = root_dir.join(format!("{}-{}", component.name, component.language));
        let bindings_dir = artifacts_dir.join("bindings");
        component
            .language
            .obj()
            .generate_bindings(self, &component.bindgen, &bindings_dir)?;
        Ok(bindings_dir)
    }

    /// Executes a single test case.
    ///
    /// Unlike upstream — which composes a `runner` and `test` guest components
    /// with WAC and runs them in a component runtime — wRPC links the `runner`
    /// (a wRPC client) and the dependency worlds (wRPC servers) into a single
    /// host binary connected over in-process TCP transports, and runs it to
    /// completion. Dependencies are chained in the order listed: each
    /// component's imports are routed to the nearest following dependency
    /// exporting them.
    fn runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        runner_bindings: &Path,
        deps: &[&(&Component, PathBuf)],
    ) -> Result<()> {
        match runner.language {
            Language::Rust => self.rust_runtime_test(case, runner, runner_bindings, deps),
            Language::Go => match deps {
                [(test, ..)] => self.go_runtime_test(case, runner, test),
                _ => bail!("Go runtime tests support exactly one `test` world"),
            },
            Language::Js => match deps {
                [(test, test_bindings)] => {
                    self.js_runtime_test(case, runner, runner_bindings, test, test_bindings)
                }
                _ => bail!("JavaScript runtime tests support exactly one `test` world"),
            },
        }
    }

    /// Helper to execute an external process and generate a helpful error
    /// message on failure.
    fn run_command(&self, cmd: &mut Command) -> Result<()> {
        if self.opts.inherit_stderr {
            cmd.stderr(Stdio::inherit());
        }
        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {cmd:?}"))?;
        if output.status.success() {
            return Ok(());
        }

        let mut error = format!(
            "\
command execution failed
command: {cmd:?}
status: {}",
            output.status,
        );

        if !output.stdout.is_empty() {
            error.push_str(&format!(
                "\nstdout:\n  {}",
                String::from_utf8_lossy(&output.stdout).replace("\n", "\n  ")
            ));
        }
        if !output.stderr.is_empty() {
            error.push_str(&format!(
                "\nstderr:\n  {}",
                String::from_utf8_lossy(&output.stderr).replace("\n", "\n  ")
            ));
        }

        bail!("{error}")
    }

    /// "poor man's test output progress"
    fn update_status<T>(&self, result: &Result<T>, should_fail: bool) {
        if result.is_ok() == !should_fail {
            print!(".");
        } else {
            print!("F");
        }
        let _ = std::io::stdout().flush();
    }

    /// Returns whether `languages` is included in this testing session.
    fn include_language(&self, language: &Language) -> bool {
        // Unlike upstream, wRPC makes `--lang` optional; its in-tree harness
        // selects every language, so an empty list means "test everything".
        self.opts.languages.is_empty()
            || self
                .opts
                .languages
                .iter()
                .any(|l| l == language.obj().display())
    }

    fn render_errors<'a>(&self, results: impl Iterator<Item = StepResult<'a>>) {
        let mut failures = 0;
        for result in results {
            let err = match (result.result, result.should_fail) {
                (Ok(()), false) | (Err(_), true) => continue,
                (Err(e), false) => e,
                (Ok(()), true) => anyhow!("test should have failed, but passed"),
            };
            failures += 1;

            println!("------ Failure: {} --------", result.name);
            for (k, v) in result.metadata {
                println!("  {k}: {v}");
            }
            println!("  error: {}", format!("{err:?}").replace("\n", "\n  "));
        }

        if failures > 0 {
            println!("{failures} tests FAILED");
            std::process::exit(1);
        }
    }
}

struct StepResult<'a> {
    result: Result<()>,
    should_fail: bool,
    name: &'a str,
    metadata: Vec<(&'a str, String)>,
}

impl<'a> StepResult<'a> {
    fn new(name: &'a str, result: Result<()>) -> StepResult<'a> {
        StepResult {
            name,
            result,
            should_fail: false,
            metadata: Vec::new(),
        }
    }

    fn should_fail(mut self, fail: bool) -> Self {
        self.should_fail = fail;
        self
    }

    fn metadata(mut self, name: &'a str, value: impl fmt::Display) -> Self {
        self.metadata.push((name, value.to_string()));
        self
    }
}

/// Helper trait for each language to implement which encapsulates
/// language-specific logic.
trait LanguageMethods {
    /// Display name for this language, used in filenames.
    fn display(&self) -> &str;

    /// Returns the prefix that this language uses to annotate configuration in
    /// the top of source files.
    ///
    /// This should be the language's line-comment syntax followed by `@`, e.g.
    /// `//@` for Rust or `;;@` for WebAssembly Text.
    fn comment_prefix_for_test_config(&self) -> Option<&str>;

    /// Returns the extra permutations, if any, of arguments to use with codegen
    /// tests.
    ///
    /// This is used to run all codegen tests with a variety of bindings
    /// generator options. The first element in the tuple is a descriptive
    /// string that should be unique (used in file names) and the second elemtn
    /// is the list of arguments for that variant to pass to the bindings
    /// generator.
    fn codegen_test_variants(&self) -> &[(&str, &[&str])] {
        &[]
    }

    /// Performs any one-time preparation necessary for this language, such as
    /// downloading or caching dependencies.
    fn prepare(&self, runner: &mut Runner<'_>) -> Result<()>;

    /// Generates bindings for `component` into `dir`.
    ///
    /// Runs `wit-bindgen` in aa subprocess to catch failures such as panics.
    fn generate_bindings(&self, runner: &Runner<'_>, bindgen: &Bindgen, dir: &Path) -> Result<()> {
        let name = match self.bindgen_name() {
            Some(name) => name,
            None => return Ok(()),
        };
        let mut cmd = Command::new(runner.wit_bindgen);
        cmd.arg(name)
            .arg(&bindgen.wit_path)
            .arg("--world")
            .arg(format!("%{}", bindgen.world))
            .arg("--out-dir")
            .arg(dir);

        match bindgen.wit_config.default_bindgen_args {
            Some(true) | None => {
                for arg in self.default_bindgen_args() {
                    cmd.arg(arg);
                }
            }
            Some(false) => {}
        }

        for arg in bindgen.args.iter() {
            cmd.arg(arg);
        }

        runner.run_command(&mut cmd)
    }

    /// Returns the default set of arguments that will be passed to
    /// `wit-bindgen`.
    ///
    /// Defaults to empty, but each language can override it.
    fn default_bindgen_args(&self) -> &[&str] {
        &[]
    }

    /// Returns the name of this bindings generator when passed to
    /// `wit-bindgen`.
    ///
    /// By default this is `Some(self.display())`, but it can be overridden if
    /// necessary. Returning `None` here means that no bindings generator is
    /// supported.
    fn bindgen_name(&self) -> Option<&str> {
        Some(self.display())
    }

    /// Returns whether this language is supposed to fail this codegen tests
    /// given the `config` and `args` for the test.
    fn should_fail_verify(&self, name: &str, config: &config::WitConfig, args: &[String]) -> bool;

    /// Returns whether this language is expected to fail the named runtime
    /// test, either while generating bindings or while running it. A test
    /// listed here that passes is reported as a failure.
    fn should_fail_runtime(&self, name: &str) -> bool;

    /// Performs a "check" or a verify that the generated bindings described by
    /// `Verify` are indeed valid.
    fn verify(&self, runner: &Runner<'_>, verify: &Verify) -> Result<()>;
}

impl Language {
    const ALL: &[Language] = &[Language::Rust, Language::Go, Language::Js];

    fn obj(&self) -> &dyn LanguageMethods {
        match self {
            Language::Rust => &rust::Rust,
            Language::Go => &go::Go,
            Language::Js => &js::Js,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.obj().display().fmt(f)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Runner => "runner".fmt(f),
            Kind::Test => "test".fmt(f),
        }
    }
}

/// Returns `true` if the file was written, or `false` if the file is the same
/// as it was already on disk.
fn write_if_different(path: &Path, contents: impl AsRef<[u8]>) -> Result<bool> {
    let contents = contents.as_ref();
    if let Ok(prev) = fs::read(path)
        && prev == contents
    {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {parent:?}"))?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {path:?}"))?;
    Ok(true)
}
