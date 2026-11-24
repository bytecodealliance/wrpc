use crate::{Bindgen, Component, LanguageMethods, Runner, Test, Verify};
use anyhow::{Context, Result};
use clap::Parser;
use core::iter;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default, Debug, Clone, Parser)]
pub struct JsOpts {
    /// Path to the in-tree `@bytecodealliance/wrpc` package that generated
    /// JavaScript bindings depend on. Defaults to `js` relative to the current
    /// directory.
    #[clap(long, value_name = "PATH")]
    js_wrpc_path: Option<PathBuf>,
}

pub struct Js;

impl LanguageMethods for Js {
    fn display(&self) -> &str {
        "js"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
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

    fn should_fail_runtime(&self, _name: &str) -> bool {
        false
    }

    fn prepare(&self, _runner: &mut Runner<'_>) -> Result<()> {
        Ok(())
    }

    fn prepare_component(&self, component: &Component, bindings_dir: &Path) -> Result<()> {
        let src = fs::read(&component.path)?;
        crate::write_if_different(&bindings_dir.join("impl.js"), src)?;
        Ok(())
    }

    fn generate_bindings(&self, runner: &Runner<'_>, bindgen: &Bindgen, dir: &Path) -> Result<()> {
        let js = runner.js_wrpc_path()?;
        let mut cmd = Command::new(runner.wit_bindgen);
        cmd.arg("js")
            .arg(&bindgen.wit_path)
            .arg("--world")
            .arg(format!("%{}", bindgen.world))
            .arg("--out-dir")
            .arg(dir)
            .arg("--wrpc-path")
            .arg(js.join("src/index.js"));
        if bindgen.wit_config.default_bindgen_args != Some(false) {
            cmd.arg("--generate-all");
        }
        cmd.args(&bindgen.args);
        runner.run_command(&mut cmd)
    }

    fn verify(&self, runner: &Runner<'_>, verify: &Verify<'_>) -> Result<()> {
        let mut modules = Vec::default();
        find_modules(verify.bindings_dir, verify.bindings_dir, &mut modules)?;
        modules.sort();
        let check = modules
            .iter()
            .map(|m| format!("import \"./bindings/{m}\";\n"))
            .collect::<String>();
        crate::write_if_different(&verify.artifacts_dir.join("check.js"), check)?;
        runner
            .run_command(
                Command::new("node")
                    .arg("check.js")
                    .current_dir(verify.artifacts_dir),
            )
            .context("failed to load generated JavaScript bindings")?;

        let js = runner.js_wrpc_path()?;
        let Some(tsc) = iter::once(js.join("node_modules/.bin"))
            .chain(env::var_os("PATH").iter().flat_map(env::split_paths))
            .map(|dir| dir.join("tsc"))
            .find(|tsc| tsc.is_file())
        else {
            return Ok(());
        };
        crate::write_if_different(
            &verify.artifacts_dir.join("tsconfig.json"),
            r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "lib": ["ES2022", "DOM"],
    "strict": true,
    "noEmit": true,
    "types": []
  },
  "include": ["bindings"]
}
"#,
        )?;
        runner
            .run_command(
                Command::new(tsc)
                    .arg("-p")
                    .arg(".")
                    .current_dir(verify.artifacts_dir),
            )
            .context("failed to type-check generated TypeScript declarations")
    }
}

/// The `file:` URL of the absolute `path`.
fn file_url(path: &Path) -> String {
    let path = path
        .display()
        .to_string()
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('?', "%3F");
    format!("file://{path}")
}

/// Collects the paths of the `.js` files in `dir`, relative to `root`.
fn find_modules(root: &Path, dir: &Path, modules: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {dir:?}"))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            find_modules(root, &path, modules)?;
        } else if path.extension().is_some_and(|ext| ext == "js") {
            let path = path.strip_prefix(root)?;
            modules.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

impl Runner<'_> {
    fn js_wrpc_path(&self) -> Result<PathBuf> {
        let cwd = env::current_dir()?;
        Ok(match &self.opts.js.js_wrpc_path {
            Some(path) => cwd.join(path),
            None => cwd.join("js"),
        })
    }

    /// Runs a single JavaScript runtime test: serves each dependency's
    /// handlers over TCP, routing its imports to the following dependencies,
    /// and runs the `runner` against them, in one Node.js process.
    pub(crate) fn js_runtime_test(
        &self,
        case: &Test,
        runner: &Component,
        runner_bindings: &Path,
        deps: &[&(&Component, PathBuf)],
    ) -> Result<()> {
        let mut name = runner.name.clone();
        for (dep, ..) in deps {
            name.push('-');
            name.push_str(&dep.name);
        }
        let cwd = env::current_dir()?;
        let driver = cwd
            .join(&self.opts.artifacts)
            .join(&case.name)
            .join(format!("{name}-js"));

        let js = self.js_wrpc_path()?;
        let wrpc = file_url(&js.join("src/index.js"));
        let run = file_url(&runner_bindings.join("impl.js"));
        let mut imports = String::default();
        let mut serve = String::default();
        for (i, (dep, bindings)) in deps.iter().enumerate().rev() {
            let handlers = file_url(&bindings.join("impl.js"));
            let world = file_url(&bindings.join(format!("{}.js", dep.bindgen.world)));
            imports.push_str(&format!(
                "import {{ handlers as handlers{i} }} from {handlers:?};\nimport {{ handle as handle{i}, served as served{i} }} from {world:?};\n"
            ));
            serve.push_str(&format!(
                "routes = [[served{i}, await listen(handle{i}, handlers{i}(router(routes)))], ...routes];\n"
            ));
        }
        crate::write_if_different(
            &driver.join("main.js"),
            format!(
                r#"import net from "node:net";
import {{ Duplex }} from "node:stream";

import {{ Framed, Listener }} from {wrpc:?};

import {{ run }} from {run:?};
{imports}
setTimeout(() => {{
  console.error("runtime test timed out");
  process.exit(1);
}}, 10_000).unref();

/** @param {{net.Socket}} sock */
const streams = (sock) => {{
  const {{ readable, writable }} = Duplex.toWeb(sock);
  return [writable, readable];
}};

/** @param {{number}} port @returns {{Promise<net.Socket>}} */
const dial = (port) =>
  new Promise((resolve, reject) => {{
    const sock = net.connect({{ host: "::1", port, allowHalfOpen: true }}, () => resolve(sock));
    sock.once("error", reject);
  }});

/**
 * A client invoking each function over a new connection to the first of
 * `routes` exporting it, which `abort` resets.
 * @param {{[[string, string][], number][]}} routes
 */
const router = (routes) => async () => {{
  /** @type {{Set<net.Socket>}} */
  const socks = new Set();
  let aborted = false;
  return {{
    async invoke(codec, instance, name, params) {{
      const route = routes.find(([funcs]) =>
        funcs.some(([i, f]) => i === instance && f === name),
      );
      if (!route) throw new Error(`no component exports \`${{instance}}#${{name}}\``);
      const sock = await dial(route[1]);
      socks.add(sock);
      sock.once("close", () => socks.delete(sock));
      if (aborted) {{
        sock.resetAndDestroy();
        throw new Error("invocation aborted");
      }}
      const io = new Framed(...streams(sock));
      try {{
        const [results, done] = await io.invoke(codec, instance, name, params);
        return [results, done.then(() => io.tx.writer.close(), (err) => {{
          sock.resetAndDestroy();
          throw err;
        }})];
      }} catch (err) {{
        sock.resetAndDestroy();
        throw err;
      }}
    }},
    abort() {{
      aborted = true;
      for (const sock of socks) sock.resetAndDestroy();
    }},
  }};
}};

/** @type {{net.Server[]}} */
const servers = [];

/** Serves `handlers` with `handle` on a new listener and returns its port. */
async function listen(handle, handlers) {{
  const server = net.createServer({{ allowHalfOpen: true }}, async (sock) => {{
    const lis = new Listener(...streams(sock));
    try {{
      for (let inv; (inv = await lis.accept()); ) {{
        const [{{ instance, name }}, io] = inv;
        const served = handle(handlers, instance, name, io);
        if (!served) throw new Error(`\`${{instance}}#${{name}}\` is not exported`);
        await served;
      }}
      await lis.io.tx.writer.close();
    }} catch (err) {{
      console.error("failed to serve invocation:", err);
      sock.resetAndDestroy();
    }}
  }});
  servers.push(server);
  await new Promise((resolve) => server.listen(0, "::1", () => resolve(undefined)));
  return /** @type {{net.AddressInfo}} */ (server.address()).port;
}}

/** @type {{[[string, string][], number][]}} */
let routes = [];
{serve}
try {{
  await run(router(routes));
}} finally {{
  for (const server of servers) server.close();
}}
"#
            ),
        )?;

        self.run_command(Command::new("node").arg("main.js").current_dir(&driver))
            .context("JavaScript runtime test driver failed")
    }
}
