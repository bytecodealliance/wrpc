// Calls the Rust `echo-server` world of `tests/js/wit` on the address given as
// the first argument, for `tests/js.rs`, with the bindings of its
// `echo-client` world in the directory given as the second argument.

import assert from "node:assert/strict";
import { test } from "node:test";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { Framed } from "../../js/src/index.js";
import { connect } from "./transport.js";

const [addr, bindings] = process.argv.slice(2);
const echo = await import(pathToFileURL(join(bindings, "echo-client/wrpc-test/js/echo.js")).href);

/**
 * Drain an async iterable of chunks into an array of chunks.
 * @template T @param {AsyncIterable<T>} stream @returns {Promise<T[]>}
 */
async function collect(stream) {
  const out = [];
  for await (const chunk of stream) out.push(chunk);
  return out;
}

/**
 * A null-prototype object of `props`, as records, flags and variants decode to.
 * @template {object} T @param {T} props @returns {T}
 */
const obj = (props) => Object.assign(Object.create(null), props);

/** @template T @param {T} v @returns {Promise<T>} */
const delay = (v) => new Promise((resolve) => setTimeout(() => resolve(v), 10));

const wide = Array.from({ length: 17 }, (_, i) => `f${i}`);
const VALUES = [
  {
    b: true,
    i8: -128,
    u8: 255,
    i16: -32768,
    u16: 65535,
    i32: -2147483648,
    u32: 4294967295,
    i64: -(1n << 63n),
    u64: (1n << 64n) - 1n,
    x32: 1.5,
    x64: NaN,
    c: "😀",
    s: "héllo 👋",
    wide: obj(Object.fromEntries(wide.map((name, i) => [name, i % 8 === 0]))),
    bytes: new Uint8Array([1, 2, 3]),
    strs: ["a", "", "c"],
    opt: obj({ kind: "some", value: undefined }),
    res: obj({ kind: "ok", value: 7 }),
    unit: obj({ kind: "error" }),
    vs: [
      obj({ kind: "empty" }),
      obj({ kind: "num", value: 3 }),
      obj({ kind: "str", value: "x" }),
      obj({ kind: "nested", value: obj({ kind: "some", value: undefined }) }),
      obj({ kind: "nested", value: obj({ kind: "none" }) }),
      obj({ kind: "nested", value: obj({ kind: "some", value: 9 }) }),
    ],
    color: "blue",
    small: obj({ a: true, b: false, c: true, httpOnly: true }),
    tup: [1, "one"],
    lists: [new Uint16Array(), Uint16Array.of(1, 65535)],
    favoriteNumbers: BigInt64Array.of(-(1n << 63n), 0n, 42n),
  },
  {
    b: false,
    i8: 127,
    u8: 0,
    i16: 32767,
    u16: 0,
    i32: 2147483647,
    u32: 0,
    i64: (1n << 63n) - 1n,
    u64: 0n,
    x32: NaN,
    x64: 5e-324,
    c: "\0",
    s: "",
    wide: obj(Object.fromEntries(wide.map((name) => [name, false]))),
    bytes: new Uint8Array(),
    strs: [],
    opt: obj({ kind: "none" }),
    res: obj({ kind: "error", value: "bad" }),
    unit: obj({ kind: "ok" }),
    vs: [],
    color: "red",
    small: obj({ a: false, b: false, c: false, httpOnly: false }),
    tup: [0, ""],
    lists: [],
    favoriteNumbers: new BigInt64Array(),
  },
];

test("values and async values echo", { timeout: 5000 }, async () => {
  const clt = await connect(addr);
  for (const val of VALUES) {
    assert.deepEqual(await echo.echo(clt, obj(val)), obj(val));
  }
  for (let i = 0; i < 2; i++) {
    async function* lists() {
      yield [["a"], []];
      yield delay([["b", "c"]]);
    }
    const [out, io] = await echo.echoAsync(clt, {
      bytes: [new Uint8Array([1]), new Uint8Array([2, i])],
      lists: lists(),
      str: delay("later"),
      nested: () => delay(() => delay(i)),
    });
    const [bytes, ls, str, nested] = await Promise.all([
      collect(out.bytes),
      collect(out.lists),
      out.str,
      out.nested,
      io,
    ]);
    assert.deepEqual(
      bytes.flatMap((chunk) => [...chunk]),
      [1, 2, i],
    );
    assert.deepEqual(ls.flat(), [["a"], [], ["b", "c"]]);
    assert.equal(str, "later");
    assert.equal(nested, i);
  }
  if (clt instanceof Framed) {
    await clt.tx.writer.close();
    assert.equal((await clt.rx.reader.read()).done, true);
  }
});
