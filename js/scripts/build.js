import { spawn } from "node:child_process";
import { once } from "node:events";
import { copyFile } from "node:fs/promises";
import { createInterface } from "node:readline";

const cargo = spawn(
  "cargo",
  [
    "build",
    "--release",
    "--target",
    "wasm32-wasip1",
    "--bin",
    "wit-bindgen-wrpc",
    "--no-default-features",
    "--features",
    "bin-bindgen",
    "--config",
    "profile.release.strip=true",
    "--message-format",
    "json-render-diagnostics",
  ],
  {
    cwd: new URL("../..", import.meta.url),
    stdio: ["inherit", "pipe", "inherit"],
  },
);
const closed = once(cargo, "close");

let wasm;
for await (const line of createInterface({ input: cargo.stdout })) {
  const msg = JSON.parse(line);
  if (msg.reason === "compiler-artifact" && msg.target.name === "wit-bindgen-wrpc") {
    wasm = msg.executable;
  }
}
const [code] = await closed;
if (code !== 0) {
  process.exit(code ?? 1);
}
if (!wasm) {
  throw new Error("cargo did not report a `wit-bindgen-wrpc` executable");
}
await copyFile(wasm, new URL("../bin/wit-bindgen-wrpc.wasm", import.meta.url));
