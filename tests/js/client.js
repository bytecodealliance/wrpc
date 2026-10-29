// Calls the Rust `echo-server` world of `tests/js/wit` on the address given as
// the first argument, for `tests/js.rs`.

import assert from "node:assert/strict";
import { test } from "node:test";

import { invoke, t } from "../../js/src/index.js";
import { connect } from "./tcp.js";

/**
 * @typedef {import("../../js/src/index.js").Chan<import("../../js/src/index.js").Chunk>} Chan
 * @typedef {import("../../js/src/index.js").InputValue} InputValue
 * @typedef {import("../../js/src/index.js").Type} Type
 */

const addr = process.argv[2];

/**
 * Send `v` of type `ty` to `func` and return what it echoes.
 * @param {string} func @param {Type} ty @param {InputValue} v
 */
async function echo(func, ty, v) {
  const { results, done, finished } = await invoke(
    await connect(addr),
    "wrpc-test:js/echo",
    func,
    [ty],
    [v],
    [ty],
  );
  return { out: results[0], done, finished };
}

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

test("values", { timeout: 5000 }, async () => {
  const wide = Array.from({ length: 17 }, (_, i) => `f${i}`);
  const v = t.variant({ empty: null, num: t.u32, str: t.string, nested: t.option(t.option(t.u8)) });
  const ty = t.record({
    b: t.bool,
    i8: t.s8,
    u8: t.u8,
    i16: t.s16,
    u16: t.u16,
    i32: t.s32,
    u32: t.u32,
    i64: t.s64,
    u64: t.u64,
    x32: t.f32,
    x64: t.f64,
    c: t.char,
    s: t.string,
    wide: t.flags(wide),
    bytes: t.list(t.u8),
    strs: t.list(t.string),
    opt: t.option(t.option(t.string)),
    res: t.result(t.u32, t.string),
    unit: t.result(),
    vs: t.list(v),
    color: t.enum(["red", "green", "blue"]),
    small: t.flags(["a", "b", "c", "HTTP-only"]),
    tup: t.tuple(t.u8, t.string),
    lists: t.list(t.list(t.u16)),
    "favorite-numbers": t.list(t.s64),
  });
  for (const val of [
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
  ]) {
    const { out, done, finished } = await echo("echo", ty, obj(val));
    assert.deepEqual(out, obj(val));
    await Promise.all([done, finished]);
  }
});

test("async", { timeout: 5000 }, async () => {
  const ty = t.record({
    bytes: t.stream(t.u8),
    lists: t.stream(t.list(t.string)),
    str: t.future(t.string),
    nested: t.future(t.future(t.u32)),
  });
  async function* lists() {
    yield [["a"], []];
    yield delay([["b", "c"]]);
  }
  const echoed = await echo("echo-async", ty, {
    bytes: [new Uint8Array([1]), new Uint8Array([2, 3])],
    lists: lists(),
    str: delay("later"),
    nested: () => delay(() => delay(42)),
  });
  const { done, finished } = echoed;
  const out =
    /** @type {{ bytes: Chan, lists: Chan, str: Promise<string>, nested: Promise<number> }} */ (
      echoed.out
    );
  const [bytes, ls, str, nested] = await Promise.all([
    collect(out.bytes),
    collect(out.lists),
    out.str,
    out.nested,
  ]);
  assert.deepEqual(
    bytes.flatMap((chunk) => [...chunk]),
    [1, 2, 3],
  );
  assert.deepEqual(ls.flat(), [["a"], [], ["b", "c"]]);
  assert.equal(str, "later");
  assert.equal(nested, 42);
  await Promise.all([done, finished]);
});
