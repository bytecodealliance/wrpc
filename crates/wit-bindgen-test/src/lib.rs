use anyhow::{bail, Context, Result};
use clap::Parser;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod config;
mod go;
mod rust;

/// Tool to run tests that exercise the wRPC bindings generators.
///
/// This tool is used to (a) generate bindings for a target language and (b)
/// either compile those bindings to check that they are valid (codegen tests)
/// or compile and run a `runner` against a `test` over an in-process wRPC
/// transport (runtime tests). This process is guided by filesystem structure
/// which must adhere to some conventions.
///
/// * Codegen tests are `*.wit` files (or directories containing a `wit/`
///   subdirectory). Bindings are generated for the single world within and then
///   compiled by the target language to ensure that valid bindings were
///   generated.
///
/// * Runtime tests are directories containing a `test.wit` file. This file must
///   have a `runner` world (which imports functionality) and a `test` world
///   (which exports it). Adjacent `runner*.*` and `test*.*` source files
///   implement those worlds. Unlike upstream `wit-bindgen` — whose bindings are
///   guest components composed in a component runtime — wRPC bindings are
///   `Invoke`/`Serve` RPC stubs, so the `runner` (a wRPC client) and the `test`
///   (a wRPC server) are linked into a single host binary, connected over an
///   in-process TCP transport, and run to completion.
///
/// Source files can start with `//@ ...` comments to deserialize into
/// per-file configuration, currently the `args` passed to the bindings
/// generator:
///
/// ```text
/// //@ args = ['--additional_derive_ignore=ignoreme']
/// ```
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

    /// Path to a file listing codegen tests whose failure is tolerated, one
    /// `<language> <test-name>` pair per line (`#` comments allowed). These are
    /// the tests the generator is known not to support yet; they still run but
    /// do not fail the suite.
    #[clap(long, value_name = "PATH")]
    allow_fail_list: Option<PathBuf>,

    #[clap(flatten)]
    rust: rust::RustOpts,

    #[clap(flatten)]
    go: go::GoOpts,

    /// Whether or not the calling process's stderr is inherited into child
    /// processes.
    ///
    /// This helps preserving color in compiler error messages but can also
    /// jumble up output if there are multiple errors.
    #[clap(short, long)]
    inherit_stderr: bool,

    /// Configuration of which languages are tested.
    ///
    /// Passing `--languages rust` will only test Rust for example. Passing
    /// `--languages=-rust` will test everything except Rust.
    #[clap(short, long)]
    languages: Vec<String>,
}

impl Opts {
    /// Runs all discovered tests, generating bindings with the `wit_bindgen`
    /// executable provided (typically the path to the calling `wit-bindgen-wrpc`
    /// binary itself).
    pub fn run(&self, wit_bindgen: &Path) -> Result<()> {
        let allow_fail = self.load_allow_fail()?;
        Runner {
            opts: self,
            rust_state: None,
            wit_bindgen,
            allow_fail,
        }
        .run()
    }

    /// Loads the `--allow-fail-list` file into a map of language to test names.
    fn load_allow_fail(&self) -> Result<HashMap<String, HashSet<String>>> {
        let mut map: HashMap<String, HashSet<String>> = HashMap::new();
        let Some(path) = &self.allow_fail_list else {
            return Ok(map);
        };
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read allow-fail list {path:?}"))?;
        for line in contents.lines() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let lang = parts
                .next()
                .context("missing language in allow-fail list")?;
            let name = parts
                .next()
                .context("missing test name in allow-fail list")?;
            map.entry(lang.to_string())
                .or_default()
                .insert(name.to_string());
        }
        Ok(map)
    }
}

/// Helper structure representing a discovered test.
struct Test {
    /// The name of this test, unique amongst all tests.
    ///
    /// Inferred from the directory or file name.
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
    name: String,

    /// The path to the source file for this component.
    path: PathBuf,

    /// Whether or not this component is a "runner" or a "test".
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
    /// Configuration found in `wit_path`.
    wit_config: config::WitConfig,
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Runner,
    Test,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Language {
    Rust,
    Go,
}

/// Helper structure to package up arguments when sent to language-specific
/// backends for `LanguageMethods::verify`.
struct Verify<'a> {
    bindings_dir: &'a Path,
    artifacts_dir: &'a Path,
}

/// Helper structure to package up runtime state associated with executing tests.
struct Runner<'a> {
    opts: &'a Opts,
    rust_state: Option<rust::State>,
    wit_bindgen: &'a Path,
    /// Map of language display name to the set of codegen test names whose
    /// failure is tolerated. Loaded from `--allow-fail-list`.
    allow_fail: HashMap<String, HashSet<String>>,
}

impl Runner<'_> {
    /// Executes all tests.
    fn run(&mut self) -> Result<()> {
        // First step, discover all tests in the specified test directories.
        let mut tests = HashMap::new();
        for test in &self.opts.test {
            self.discover_tests(&mut tests, test)
                .with_context(|| format!("failed to discover tests in {test:?}"))?;
        }
        if tests.is_empty() {
            bail!("no tests found in {:?}", self.opts.test);
        }

        self.prepare_languages(&tests)?;
        self.run_codegen_tests(&tests)?;
        self.run_runtime_tests(&tests)?;

        println!("PASSED");

        Ok(())
    }

    /// Walks over `path`, recursively, inserting located cases into `tests`.
    fn discover_tests(&self, tests: &mut HashMap<String, Test>, path: &Path) -> Result<()> {
        if path.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("wit") {
                return self.insert_test(path, TestKind::Codegen(path.to_owned()), tests);
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
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("non-utf-8 filename")?;
        // Codegen tests are `*.wit` files; drop the extension so test names are
        // stable whether the test is a single file or a directory.
        let test_name = file_name.strip_suffix(".wit").unwrap_or(file_name);
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

    /// Loads a runtime test from `dir` using the `test.wit` file specified.
    ///
    /// Returns a list of components that were found within this directory.
    fn load_test(&self, wit: &Path, dir: &Path) -> Result<Vec<Component>> {
        // The `runner`/`test` worlds are validated by the bindings generator
        // when it is invoked below; no WIT parsing is done here so this crate
        // stays decoupled from the `wit-parser` version under test.
        let wit_contents = fs::read_to_string(wit)?;
        let wit_config: config::WitConfig = config::parse_test_config(&wit_contents, "//@")
            .context("failed to parse WIT test config")?;

        let mut components = Vec::new();
        let mut any_runner = false;
        let mut any_test = false;

        for entry in dir.read_dir().context("failed to read test directory")? {
            let entry = entry.context("failed to read test directory entry")?;
            let path = entry.path();

            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let kind = if name.starts_with("runner") {
                any_runner = true;
                Kind::Runner
            } else if name != "test.wit" && name.starts_with("test") {
                any_test = true;
                Kind::Test
            } else {
                continue;
            };

            let bindgen = Bindgen {
                args: Vec::new(),
                wit_config: wit_config.clone(),
                world: kind.to_string(),
                wit_path: wit.to_path_buf(),
            };

            let component = self
                .parse_component(&path, kind, bindgen)
                .with_context(|| format!("failed to parse component source file {path:?}"))?;
            components.push(component);
        }

        if !any_runner {
            bail!("no `runner*` test files found in test directory");
        }
        if !any_test {
            bail!("no `test*` test files found in test directory");
        }

        Ok(components)
    }

    /// Parses the component located at `path` and creates all information
    /// necessary for a `Component` return value.
    fn parse_component(&self, path: &Path, kind: Kind, mut bindgen: Bindgen) -> Result<Component> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .context("non-utf-8 path extension")?;

        let language = match extension {
            "rs" => Language::Rust,
            "go" => Language::Go,
            other => bail!("unsupported test source file extension `{other}`"),
        };

        let contents = fs::read_to_string(path)?;
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

    /// Prepares all languages in use as part of a one-time initialization step.
    fn prepare_languages(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let all_languages = Language::ALL.to_vec();

        let mut prepared = HashSet::new();
        let mut to_prepare = Vec::new();
        let mut consider = |lang: &Language, prepared: &mut HashSet<Language>| {
            if self.include_language(lang) && prepared.insert(lang.clone()) {
                to_prepare.push(lang.clone());
            }
        };

        for test in tests.values() {
            match &test.kind {
                TestKind::Runtime(c) => {
                    for component in c {
                        consider(&component.language, &mut prepared);
                    }
                }
                TestKind::Codegen(_) => {
                    for lang in &all_languages {
                        consider(lang, &mut prepared);
                    }
                }
            }
        }

        for lang in to_prepare {
            lang.obj()
                .prepare(self)
                .with_context(|| format!("failed to prepare language {lang}"))?;
        }

        Ok(())
    }

    /// Executes all tests that are `TestKind::Codegen`.
    fn run_codegen_tests(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let mut codegen_tests = Vec::new();
        let languages = Language::ALL.to_vec();
        for (name, test) in tests.iter().filter_map(|(name, t)| match &t.kind {
            TestKind::Runtime(_) => None,
            TestKind::Codegen(p) => Some((name, p)),
        }) {
            if let Some(filter) = &self.opts.filter {
                if !filter.is_match(name) {
                    continue;
                }
            }
            let config = match fs::read_to_string(test) {
                Ok(wit) => config::parse_test_config::<config::WitConfig>(&wit, "//@")
                    .with_context(|| format!("failed to parse test config from {test:?}"))?,
                Err(_) => Default::default(),
            };
            for language in &languages {
                // If the CLI arguments filter out this language, then discard
                // the test case.
                if !self.include_language(language) {
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
                        args.iter().map(ToString::to_string).collect::<Vec<_>>(),
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
                let allow_fail = self.allow_fail(language, args_kind);
                let result = self
                    .codegen_test(language, test, args, config)
                    .with_context(|| {
                        format!("failed to codegen test for `{language}` over {test:?}")
                    });
                self.update_status(&result, allow_fail);
                (result, allow_fail, language, test, args_kind)
            })
            .collect::<Vec<_>>();

        println!();

        self.render_errors(results.into_iter().map(
            |(result, allow_fail, language, test, args_kind)| {
                StepResult::new(test.to_str().unwrap(), result)
                    .allow_fail(allow_fail)
                    .metadata("language", language)
                    .metadata("variant", args_kind)
            },
        ));

        Ok(())
    }

    /// Runs a single codegen test: generate bindings for `test` in `language`
    /// and verify they compile.
    fn codegen_test(
        &self,
        language: &Language,
        test: &Path,
        args: &[String],
        config: &config::WitConfig,
    ) -> Result<()> {
        let artifacts_dir = std::env::current_dir()?
            .join(&self.opts.artifacts)
            .join("codegen")
            .join(language.to_string())
            .join(sanitize(test));
        let bindings_dir = artifacts_dir.join("bindings");
        // An empty world lets the generator auto-select the single world in the
        // file (avoids parsing WIT here, and sidesteps `--world` escaping for
        // keyword-named worlds).
        let bindgen = Bindgen {
            args: args.to_vec(),
            wit_path: test.to_path_buf(),
            world: String::new(),
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
                    artifacts_dir: &artifacts_dir,
                    bindings_dir: &bindings_dir,
                },
            )
            .context("failed to verify generated bindings")?;

        Ok(())
    }

    /// Execute all `TestKind::Runtime` tests.
    ///
    /// Unlike upstream, the `runner` (client) and `test` (server) are linked
    /// into one host binary and connected over an in-process TCP transport, so
    /// the unit of work is a `(runner, test)` pair rather than a composed
    /// component.
    fn run_runtime_tests(&mut self, tests: &HashMap<String, Test>) -> Result<()> {
        let mut to_run = Vec::new();
        for test in tests.values() {
            if let Some(filter) = &self.opts.filter {
                if !filter.is_match(&test.name) {
                    continue;
                }
            }
            let TestKind::Runtime(components) = &test.kind else {
                continue;
            };
            for runner in components
                .iter()
                .filter(|c| c.kind == Kind::Runner && self.include_language(&c.language))
            {
                for tst in components
                    .iter()
                    .filter(|c| c.kind == Kind::Test && c.language == runner.language)
                {
                    to_run.push((test, runner, tst));
                }
            }
        }

        if to_run.is_empty() {
            return Ok(());
        }

        println!("Running {} runtime tests:", to_run.len());

        let results = to_run
            .par_iter()
            .map(|(test, runner, tst)| {
                let result = self.runtime_test(test, runner, tst).with_context(|| {
                    format!(
                        "failed to run `{}` with runner `{}` and test `{}`",
                        test.name, runner.name, tst.name,
                    )
                });
                self.update_status(&result, false);
                (result, test, runner, tst)
            })
            .collect::<Vec<_>>();

        println!();

        self.render_errors(results.into_iter().map(|(result, test, runner, tst)| {
            StepResult::new(&test.name, result)
                .metadata("runner", runner.path.display())
                .metadata("test", tst.path.display())
        }));

        Ok(())
    }

    /// Executes a single runtime test case by linking the `runner` and `test`
    /// into one binary and running it.
    fn runtime_test(&self, test: &Test, runner: &Component, tst: &Component) -> Result<()> {
        match runner.language {
            Language::Rust => self.rust_runtime_test(test, runner, tst),
            Language::Go => bail!("Go runtime tests are not yet supported"),
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
                String::from_utf8_lossy(&output.stdout).replace('\n', "\n  ")
            ));
        }
        if !output.stderr.is_empty() {
            error.push_str(&format!(
                "\nstderr:\n  {}",
                String::from_utf8_lossy(&output.stderr).replace('\n', "\n  ")
            ));
        }

        bail!("{error}")
    }

    /// "poor man's test output progress"
    fn update_status<T>(&self, result: &Result<T>, allow_fail: bool) {
        if result.is_err() && !allow_fail {
            print!("F");
        } else {
            print!(".");
        }
        let _ = std::io::stdout().flush();
    }

    /// Returns whether a codegen test failure is tolerated for `language`,
    /// consulting the optional `--allow-fail-list` file. These are the tests
    /// that the bindings generator is known not to support yet (the per-version
    /// skip list inherited from the previous macro-based codegen harness); they
    /// still run, but failing them is not an error.
    fn allow_fail(&self, language: &Language, name: &str) -> bool {
        self.allow_fail
            .get(language.obj().display())
            .is_some_and(|set| set.contains(name))
    }

    /// Returns whether `language` is included in this testing session.
    fn include_language(&self, language: &Language) -> bool {
        let lang = language.obj().display();
        let mut any_positive = false;
        let mut any_negative = false;
        for opt in &self.opts.languages {
            for name in opt.split(',') {
                if let Some(suffix) = name.strip_prefix('-') {
                    any_negative = true;
                    // If explicitly asked to not include this, don't include it.
                    if suffix == lang {
                        return false;
                    }
                } else {
                    any_positive = true;
                    // If explicitly asked to include this, then include it.
                    if name == lang {
                        return true;
                    }
                }
            }
        }

        // By default include all languages.
        if self.opts.languages.is_empty() {
            return true;
        }

        // If any language was explicitly included then assume any non-mentioned
        // language should be omitted.
        if any_positive {
            return false;
        }

        // And if there are only negative mentions (e.g. `-foo`) then assume
        // everything else is allowed.
        assert!(any_negative);
        true
    }

    fn render_errors<'a>(&self, results: impl Iterator<Item = StepResult<'a>>) {
        let mut failures = 0;
        for result in results {
            let err = match (result.result, result.allow_fail) {
                (Ok(()), _) | (Err(_), true) => continue,
                (Err(e), false) => e,
            };
            failures += 1;

            println!("------ Failure: {} --------", result.name);
            for (k, v) in result.metadata {
                println!("  {k}: {v}");
            }
            println!("  error: {}", format!("{err:?}").replace('\n', "\n  "));
        }

        if failures > 0 {
            println!("{failures} tests FAILED");
            std::process::exit(1);
        }
    }
}

struct StepResult<'a> {
    result: Result<()>,
    allow_fail: bool,
    name: &'a str,
    metadata: Vec<(&'a str, String)>,
}

impl<'a> StepResult<'a> {
    fn new(name: &'a str, result: Result<()>) -> StepResult<'a> {
        StepResult {
            name,
            result,
            allow_fail: false,
            metadata: Vec::new(),
        }
    }

    fn allow_fail(mut self, allow: bool) -> Self {
        self.allow_fail = allow;
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
    /// the top of source files (its line-comment syntax followed by `@`).
    fn comment_prefix_for_test_config(&self) -> Option<&str>;

    /// Returns the extra permutations, if any, of arguments to use with codegen
    /// tests.
    fn codegen_test_variants(&self) -> &[(&str, &[&str])] {
        &[]
    }

    /// Performs any one-time preparation necessary for this language, such as
    /// building the runtime crate that generated bindings depend on.
    fn prepare(&self, runner: &mut Runner<'_>) -> Result<()>;

    /// Generates bindings for `bindgen` into `dir`.
    ///
    /// Runs `wit-bindgen-wrpc` in a subprocess to catch failures such as panics.
    fn generate_bindings(&self, runner: &Runner<'_>, bindgen: &Bindgen, dir: &Path) -> Result<()> {
        let name = match self.bindgen_name() {
            Some(name) => name,
            None => return Ok(()),
        };
        // `world` is escaped with `%` so keyword-named worlds (e.g. `type`)
        // parse as identifiers.
        let build = |world: Option<&str>| {
            let mut cmd = Command::new(runner.wit_bindgen);
            cmd.arg(name).arg(&bindgen.wit_path);
            if let Some(world) = world {
                cmd.arg("--world").arg(format!("%{world}"));
            }
            cmd.arg("--out-dir").arg(dir);
            match bindgen.wit_config.default_bindgen_args {
                Some(true) | None => {
                    for arg in self.default_bindgen_args() {
                        cmd.arg(arg);
                    }
                }
                Some(false) => {}
            }
            for arg in &bindgen.args {
                cmd.arg(arg);
            }
            cmd
        };

        if !bindgen.world.is_empty() {
            // Runtime tests select an explicit world.
            return runner.run_command(&mut build(Some(&bindgen.world)));
        }

        // Codegen tests let the generator auto-select the single world, falling
        // back to the conventional `imports` world for multi-world packages
        // (e.g. `wasi:http`). This avoids parsing WIT here.
        if runner.run_command(&mut build(None)).is_ok() {
            return Ok(());
        }
        runner.run_command(&mut build(Some("imports")))
    }

    /// Returns the default set of arguments that will be passed to
    /// `wit-bindgen-wrpc`.
    fn default_bindgen_args(&self) -> &[&str] {
        &[]
    }

    /// Returns the name of this bindings generator when passed to
    /// `wit-bindgen-wrpc` (e.g. the `rust`/`go` subcommand).
    fn bindgen_name(&self) -> Option<&str> {
        Some(self.display())
    }

    /// Performs a "check" that the generated bindings described by `Verify` are
    /// indeed valid.
    fn verify(&self, runner: &Runner<'_>, verify: &Verify<'_>) -> Result<()>;
}

impl Language {
    const ALL: &'static [Language] = &[Language::Rust, Language::Go];

    fn obj(&self) -> &dyn LanguageMethods {
        match self {
            Language::Rust => &rust::Rust,
            Language::Go => &go::Go,
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

/// Turns a path into a filesystem-safe stable component used in artifact
/// directory names.
fn sanitize(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Returns `true` if the file was written, or `false` if the file is the same
/// as it was already on disk.
fn write_if_different(path: &Path, contents: impl AsRef<[u8]>) -> Result<bool> {
    let contents = contents.as_ref();
    if let Ok(prev) = fs::read(path) {
        if prev == contents {
            return Ok(false);
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {parent:?}"))?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {path:?}"))?;
    Ok(true)
}
