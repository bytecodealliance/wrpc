#!/usr/bin/env node
import { readdirSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { WASI } from "node:wasi";

const preopens = {};
let args = process.argv.slice(2);
let cwd = process.cwd();
if (process.platform === "win32") {
  const toGuest = (path) =>
    path
      .replace(/^(--[^=]+=)?([A-Za-z]):(?=[\\/])/, (_, flag = "", drive) => {
        const letter = drive.toLowerCase();
        preopens[`/${letter}`] = `${letter}:\\`;
        return `${flag}/${letter}`;
      })
      .replaceAll("\\", "/");
  args = args.map(toGuest);
  cwd = toGuest(cwd);
} else {
  preopens["/"] = "/";
}

const wasi = new WASI({
  version: "preview1",
  args: ["wit-bindgen-wrpc", ...args],
  env: { ...process.env, PWD: cwd },
  preopens,
  returnOnExit: true,
});
const imports = wasi.getImportObject();
let memory;
if (process.platform === "win32") {
  const wasip1 = imports.wasi_snapshot_preview1;
  const { path_open, fd_close } = wasip1;
  const paths = new Map(Object.values(preopens).map((path, i) => [i + 3, path]));
  const listings = new Map();
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();
  wasip1.path_open = (dirfd, dirflags, ptr, len, oflags, base, inheriting, fdflags, fdPtr) => {
    const errno = path_open(dirfd, dirflags, ptr, len, oflags, base, inheriting, fdflags, fdPtr);
    if (errno === 0 && paths.has(dirfd)) {
      const path = decoder.decode(new Uint8Array(memory.buffer, ptr, len));
      paths.set(new DataView(memory.buffer).getUint32(fdPtr, true), join(paths.get(dirfd), path));
    }
    return errno;
  };
  wasip1.fd_close = (fd) => {
    paths.delete(fd);
    listings.delete(fd);
    return fd_close(fd);
  };
  wasip1.fd_readdir = (fd, buf, bufLen, cookie, bufusedPtr) => {
    const dir = paths.get(fd);
    if (dir === undefined) {
      return 8;
    }
    let entries = cookie === 0n ? undefined : listings.get(fd);
    if (entries === undefined) {
      try {
        entries = readdirSync(dir, { withFileTypes: true });
      } catch (err) {
        return { EACCES: 2, ENOENT: 44, ENOTDIR: 54, EPERM: 63 }[err.code] ?? 29;
      }
      listings.set(fd, entries);
    }
    const out = new Uint8Array(memory.buffer, buf, bufLen);
    let used = 0;
    for (let i = Number(cookie); i < entries.length && used < bufLen; i++) {
      const entry = entries[i];
      const name = encoder.encode(entry.name);
      const dirent = new Uint8Array(24 + name.length);
      const view = new DataView(dirent.buffer);
      view.setBigUint64(0, BigInt(i + 1), true);
      view.setUint32(16, name.length, true);
      view.setUint8(
        20,
        entry.isDirectory() ? 3 : entry.isFile() ? 4 : entry.isSymbolicLink() ? 7 : 0,
      );
      dirent.set(name, 24);
      const n = Math.min(dirent.length, bufLen - used);
      out.set(dirent.subarray(0, n), used);
      used += n;
    }
    new DataView(memory.buffer).setUint32(bufusedPtr, used, true);
    return 0;
  };
}
const wasm = await readFile(new URL("wit-bindgen-wrpc.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(wasm, imports);
memory = instance.exports.memory;
process.exitCode = wasi.start(instance);
