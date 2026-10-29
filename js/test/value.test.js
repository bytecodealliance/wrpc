import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

import * as t from "../src/types.js";
import { AsyncReader, Chan, Reader, Writer } from "../src/io.js";
import { encodeValue, decodeSteps, decodeValue } from "../src/value.js";

/**
 * @typedef {import("../src/index.js").Chunk} Chunk
 * @typedef {import("../src/index.js").InputValue} InputValue
 * @typedef {import("../src/index.js").Type} Type
 * @typedef {import("../src/index.js").TypedArray} TypedArray
 * @typedef {{ new (length: number): TypedArray, from(vals: Iterable<number | bigint>): TypedArray, readonly name: string }} TypedArrayConstructor
 * @typedef {import("../src/index.js").Value} Value
 */

/**
 * Encode a single value against `ty` to bytes.
 * @param {Type} ty @param {InputValue} v
 */
function encode(ty, v) {
  const w = new Writer();
  encodeValue(w, ty, v);
  return w.finish();
}

/**
 * Decode a single value of type `ty` from `bytes`.
 * @param {Type} ty @param {Uint8Array} bytes
 */
function decode(ty, bytes) {
  return decodeValue(new Reader(bytes), ty);
}

/**
 * A null-prototype object of `props`, as records, flags and variants decode to.
 * @template {object} T @param {T} props @returns {T}
 */
const obj = (props) => Object.assign(Object.create(null), props);

/**
 * Assert that `v` survives an encode/decode round-trip against `ty`.
 * @param {Type} ty @param {InputValue} v
 */
function roundtrip(ty, v) {
  const out = decode(ty, encode(ty, v));
  assert.deepEqual(out, v);
  return out;
}

test("scalars round-trip", () => {
  for (const [ty, vals] of /** @type {[Type, InputValue[]][]} */ ([
    [t.bool, [true, false]],
    [t.u8, [0, 127, 255]],
    [t.s8, [-128, -1, 127]],
    [t.u16, [0, 300, 65535]],
    [t.s16, [-32768, -1, 32767]],
    [t.u32, [0, 4294967295]],
    [t.s32, [-2147483648, -1, 2147483647]],
    [t.u64, [0n, (1n << 64n) - 1n]],
    [t.s64, [-(1n << 63n), (1n << 63n) - 1n]],
    [t.f32, [1.5, -Infinity]],
    [t.f64, [0, -2.25, Math.PI]],
    [t.char, ["a", "€", "😀"]],
    [t.string, ["", "hello, wRPC 👋", "﻿bom"]],
  ])) {
    for (const v of vals) roundtrip(ty, v);
  }
  // numbers and bigints are encoded alike across the LEB128 byte boundaries
  for (const v of [0, 63, 64, 127, 128, 8191, 8192, 2 ** 32 - 1]) {
    assert.deepEqual(encode(t.u32, v), encode(t.u64, BigInt(v)));
  }
  for (const v of [-1, 63, 64, -64, -65, 8191, 8192, -8192, -8193, -(2 ** 31)]) {
    assert.deepEqual(encode(t.s32, v), encode(t.s64, BigInt(v)));
  }
});

test("scalars are converted like the JS-API draft converts them", () => {
  for (const [ty, v, converted] of /** @type {[Type, InputValue, Value][]} */ ([
    [t.u8, 256, 0],
    [t.u8, -1, 255],
    [t.s8, 128, -128],
    [t.s16, 32768, -32768],
    [t.u32, -1, 4294967295],
    [t.s32, 2147483648, -2147483648],
    [t.u64, -1n, (1n << 64n) - 1n],
    [t.s64, 1n << 63n, -(1n << 63n)],
    [t.u8, 1.5, 1],
    [t.u32, NaN, 0],
    [t.u32, "7", 7],
    [t.u32, undefined, 0],
    [t.u64, "7", 7n],
    [t.s64, true, 1n],
    [t.bool, "false", true],
    [t.bool, undefined, false],
    [t.f64, "1.5", 1.5],
    [t.f32, 0.1, Math.fround(0.1)],
    [t.string, 1, "1"],
    [t.string, "a\uD800", "a�"],
    [t.string, "\uDE00\uD83D", "��"],
    [t.enum(["red"]), ["red"], "red"],
  ])) {
    assert.equal(decode(ty, encode(ty, v)), converted, `${ty.kind} ${String(v)}`);
  }
  for (const [ty, v] of /** @type {[Type, InputValue][]} */ ([
    [t.u8, 1n],
    [t.f64, 1n],
    [t.u32, Symbol()],
    [t.string, Symbol()],
    [t.u64, 1],
    [t.s64, undefined],
  ])) {
    assert.throws(() => encode(ty, v), TypeError, `${ty.kind} ${String(v)}`);
  }
  for (const v of ["ab", "", "\uD800"]) {
    assert.throws(() => encode(t.char, v), /single Unicode scalar/);
  }
});

test("NaNs are encoded canonically", () => {
  const nan = new DataView(new ArrayBuffer(8));
  nan.setBigUint64(0, 0x7ff0000000000001n, true);
  const payload = nan.getFloat64(0, true);
  assert.deepEqual(encode(t.f64, payload), Uint8Array.of(0, 0, 0, 0, 0, 0, 0xf8, 0x7f));
  assert.deepEqual(encode(t.f32, payload), Uint8Array.of(0, 0, 0xc0, 0x7f));
  assert.ok(Number.isNaN(decode(t.f64, encode(t.f64, NaN))));
});

test("decoding rejects invalid input", () => {
  for (const [ty, bytes, error] of /** @type {[Type, number[], RegExp][]} */ ([
    [t.u32, [0x80, 0x80, 0x80, 0x80, 0x10], /overflows/],
    [t.u32, [0x80, 0x80, 0x80, 0x80, 0x80, 0x00], /overflows/],
    [t.u16, [0x80, 0x80, 0x04], /overflows/],
    [t.s16, [0x80, 0x80, 0x02], /overflows/],
    [t.s64, new Array(11).fill(0xff), /overflows/],
    [t.s8, [], /unexpected end of input/],
    [t.bool, [2], /invalid bool value byte `2`/],
    [t.option(t.u8), [2, 0], /invalid option status byte `2`/],
    [t.result(), [0xff], /invalid result status byte `255`/],
    [t.future(t.u8), [2, 0], /invalid option status byte `2`/],
    [t.stream(t.u8), [2, 0], /invalid option status byte `2`/],
    [t.string, [1, 0xff], /encoded data was not valid/],
    [t.char, [0x80, 0x80], /encoded data was not valid/],
    [t.flags(["a", "b", "c"]), [0b1000], /invalid flags bits/],
    [t.flags(Array.from({ length: 9 }, (_, i) => `f${i}`)), [0, 2], /invalid flags bits/],
  ])) {
    assert.throws(() => decode(ty, Uint8Array.from(bytes)), error, `${ty.kind} ${bytes}`);
  }
  assert.equal(decode(t.u32, Uint8Array.of(0xff, 0xff, 0xff, 0xff, 0x0f)), 4294967295);
  assert.equal(decode(t.s32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x78)), -2147483648);
});

/**
 * Sample values of each numeric type, and the typed array lists of it map to.
 * @type {[Type, TypedArrayConstructor, number[] | bigint[]][]}
 */
const NUMERIC = [
  [t.s8, Int8Array, [-128, -1, 0, 127]],
  [t.u8, Uint8Array, [0, 1, 255]],
  [t.s16, Int16Array, [-32768, -1, 300, 32767]],
  [t.u16, Uint16Array, [0, 300, 65535]],
  [t.s32, Int32Array, [-2147483648, -1, 2147483647]],
  [t.u32, Uint32Array, [0, 300, 4294967295]],
  [t.s64, BigInt64Array, [-(1n << 63n), -1n, (1n << 63n) - 1n]],
  [t.u64, BigUint64Array, [0n, 300n, (1n << 64n) - 1n]],
  [t.f32, Float32Array, [1.5, -0.25, Infinity]],
  [t.f64, Float64Array, [Math.PI, 5e-324, -Infinity]],
];

test("numeric lists round-trip as typed arrays", () => {
  for (const [elem, Typed, vals] of NUMERIC) {
    const ty = t.list(elem);
    const typed = Typed.from(vals);
    assert.ok(roundtrip(ty, typed) instanceof Typed, `\`list<${elem.kind}>\` is \`${Typed.name}\``);
    assert.ok(roundtrip(ty, new Typed(0)) instanceof Typed);
    // encoding also accepts a plain array, and typed arrays from another realm
    assert.deepEqual(encode(ty, vals), encode(ty, typed));
    assert.deepEqual(
      encode(ty, runInNewContext(`${Typed.name}.from(vals)`, { vals })),
      encode(ty, typed),
    );
  }
  assert.ok(roundtrip(t.own("bucket"), Uint8Array.of(1, 2)) instanceof Uint8Array);
  roundtrip(t.borrow("bucket"), Uint8Array.of(1, 2));
  // other iterables are converted like the typed array constructors do
  for (const ty of [t.list(t.u8), t.own("bucket")]) {
    for (const v of [[1, 2], Int8Array.of(1, 2), new Set([1, 2]), runInNewContext("[1, 2]")]) {
      assert.deepEqual(encode(ty, v), encode(ty, Uint8Array.of(1, 2)));
    }
    assert.deepEqual(encode(ty, Uint16Array.of(0x1ff)), encode(ty, Uint8Array.of(0xff)));
    for (const v of [
      Uint8Array.of(1).buffer,
      new DataView(new ArrayBuffer(1)),
      "1",
      { length: 1 },
    ]) {
      // @ts-expect-error
      assert.throws(() => encode(ty, v), { name: "TypeError", message: /expected an iterable/ });
    }
  }
  assert.deepEqual(encode(t.list(t.s8), [128]), encode(t.list(t.s8), [-128]));
  assert.throws(() => encode(t.list(t.s64), [-1, 2]), TypeError);
  assert.throws(() => encode(t.list(t.u64), Uint8Array.of(1)), TypeError);
  const detached = Uint8Array.of(1);
  structuredClone(detached.buffer, { transfer: [detached.buffer] });
  assert.throws(() => encode(t.list(t.u8), detached), /attached buffer/);
});

test("lists, tuples, records", () => {
  roundtrip(t.list(t.string), ["a", "b", "c"]);
  roundtrip(t.list(t.list(t.u16)), [Uint16Array.of(1), new Uint16Array(0)]);
  roundtrip(t.tuple(t.u8, t.string, t.bool), [7, "x", true]);
  const rec = t.record({ keys: t.list(t.string), cursor: t.option(t.string) });
  roundtrip(rec, obj({ keys: ["a", "b"], cursor: "next" }));
  // lists and tuples are any iterables, and missing tuple elements are `undefined`
  assert.deepEqual(decode(t.list(t.string), encode(t.list(t.string), new Set(["a"]))), ["a"]);
  const tup = t.tuple(t.u8, t.option(t.string));
  assert.deepEqual(decode(tup, encode(tup, new Set([7]))), [7, undefined]);
  assert.deepEqual(decode(tup, encode(tup, [7, "x", "extra"])), [7, "x"]);
  for (const v of ["ab", 1, { 0: 1, length: 1 }]) {
    assert.throws(() => encode(tup, v), { name: "TypeError", message: /expected an iterable/ });
  }
  assert.throws(() => encode(rec, "x"), { name: "TypeError", message: /expected an object/ });
});

test("record fields are read like properties, missing ones are `undefined`", () => {
  const ty = t.record({ a: t.u8, "b-c": t.option(t.u8) });
  const bytes = encode(ty, { a: 1, bC: 2 });
  /** @type {{ a: number, bC: number }} */
  const getters = new (class {
    get a() {
      return 1;
    }
    get bC() {
      return 2;
    }
  })();
  for (const v of [
    Object.create({ a: 1, bC: 2 }),
    getters,
    runInNewContext("({ a: 1, bC: 2 })"),
    { a: 1, bC: 2, c: 3 },
  ]) {
    assert.deepEqual(encode(ty, v), bytes);
  }
  assert.deepEqual(decode(ty, encode(ty, { a: 1 })), obj({ a: 1, bC: undefined }));
  const nested = t.record({ "nested-option": t.option(t.option(t.u8)) });
  assert.throws(() => encode(nested, {}), {
    name: "TypeError",
    message: "expected `{ kind, value? }` for a nested `option`",
  });
});

test("names are converted to camel case like the JS-API draft converts them", () => {
  for (const [name, key] of [
    ["kebab-case", "kebabCase"],
    ["URL", "url"],
    ["XML-http-request", "xmlHttpRequest"],
    ["get-URL-path", "getURLPath"],
    ["inner-HTML", "innerHTML"],
    ["HTTP-error", "httpError"],
    ["status-404", "status404"],
    ["v2-api", "v2Api"],
    ["a-b-c", "aBC"],
    ["default", "default"],
  ]) {
    assert.equal(t.record({ [name]: t.u8 }).fields[0].key, key, name);
    assert.equal(t.flags([name]).flags[0].key, key, name);
  }
  assert.throws(() => t.record({ "a-b": t.u8, "a-B": t.u8 }), {
    name: "TypeError",
    message: "record fields `a-b` and `a-B` both have the key `aB`",
  });
  assert.throws(() => t.flags(["http-only", "HTTP-only"]), {
    name: "TypeError",
    message: "flags `http-only` and `HTTP-only` both have the key `httpOnly`",
  });
});

test("options", () => {
  const ty = t.option(t.u32);
  roundtrip(ty, 42);
  assert.equal(decode(ty, encode(ty, undefined)), undefined);
  assert.equal(decode(ty, encode(ty, null)), undefined);
  // nested options keep some(none) distinct from none
  const nested = t.option(t.option(t.u32));
  roundtrip(nested, obj({ kind: "some", value: 1 }));
  roundtrip(nested, obj({ kind: "some", value: undefined }));
  roundtrip(nested, obj({ kind: "none" }));
  assert.deepEqual(encode(nested, { kind: "none", value: 1 }), Uint8Array.of(0));
  assert.throws(() => encode(nested, undefined), /nested `option`/);
  assert.throws(() => encode(nested, { tag: "none" }), /unknown case `undefined`/);
});

test("results", () => {
  const ty = t.result(t.string, t.u32);
  roundtrip(ty, obj({ kind: "ok", value: "fine" }));
  roundtrip(ty, obj({ kind: "error", value: 7 }));
  assert.throws(() => encode(ty, { kind: "err", value: 7 }), {
    name: "TypeError",
    message: "unknown case `err` of a `result`",
  });
  // `result<_, e>`: ok carries no payload, so it has no `value`
  const unit = t.result(null, t.string);
  assert.deepEqual(decode(unit, encode(unit, { kind: "ok", value: 1 })), obj({ kind: "ok" }));
});

test("enums and variants", () => {
  const color = t.enum(["red", "green", "blue"]);
  roundtrip(color, "green");
  assert.throws(() => encode(color, "pink"), /unknown enum case `pink`/);
  const err = t.variant({ "no-such-store": null, other: t.string });
  roundtrip(err, obj({ kind: "no-such-store" }));
  roundtrip(err, obj({ kind: "other", value: "boom" }));
  // the case is converted to a string, and the payload is read only if the case has one
  const kind = { toString: () => "other" };
  assert.deepEqual(encode(err, { kind, value: "x" }), encode(err, { kind: "other", value: "x" }));
  assert.deepEqual(encode(err, { kind: "no-such-store", value: 1 }), Uint8Array.of(0));
  assert.throws(() => encode(err, "other"), /expected `{ kind, value\? }` for a `variant`/);
  assert.throws(() => encode(err, { kind: "nope" }), /unknown case `nope` of a `variant`/);
});

test("flags", () => {
  const ty = t.flags(["read", "write", "exec"]);
  roundtrip(ty, obj({ read: true, write: false, exec: true }));
  // unset flags may be left out, flags are converted to booleans and read
  // like properties, and other properties are ignored
  assert.deepEqual(
    decode(ty, encode(ty, { read: 1, write: "", exec: "yes", wirte: true })),
    obj({ read: true, write: false, exec: true }),
  );
  assert.deepEqual(encode(ty, Object.create({ exec: true })), Uint8Array.of(0b100));
  for (const v of [null, "read", 1]) {
    assert.throws(() => encode(ty, v), /expected an object for `flags`/);
  }
  const wide = t.flags(Array.from({ length: 9 }, (_, i) => `f${i}`));
  assert.equal(decode(wide, Uint8Array.of(0, 1)).f8, true);
});

test("streams and futures are decoded synchronously only if ready", async () => {
  for (const [ty, v] of /** @type {[Type, InputValue][]} */ ([
    [t.stream(t.u8), Uint8Array.of(1)],
    [t.future(t.string), "x"],
  ])) {
    assert.throws(() => encode(ty, v), /async session API/);
    assert.throws(() => decode(ty, Uint8Array.of(0)), /async session API/);
  }
  /** @param {Value} chunks */
  const collect = async (chunks) => {
    const out = [];
    for await (const chunk of /** @type {AsyncIterable<Chunk>} */ (chunks)) out.push(chunk);
    return out;
  };
  for (const [ty, bytes, chunks] of /** @type {[Type, number[], Chunk[]][]} */ ([
    [t.stream(t.u8), [1, 2, 7, 8], [Uint8Array.of(7, 8)]],
    [t.stream(t.u16), [1, 1, 0x80, 0x02], [Uint16Array.of(256)]],
    [t.stream(t.string), [1, 1, 1, 0x61], [["a"]]],
    [t.stream(t.u8), [1, 0], []],
  ])) {
    assert.deepEqual(await collect(decode(ty, Uint8Array.from(bytes))), chunks);
  }
  assert.equal(await decode(t.future(t.u8), Uint8Array.of(1, 7)), 7);
  assert.equal(await decode(t.future(), Uint8Array.of(1)), undefined);
});

test("values decode resumably across chunk boundaries", async () => {
  const strings = Array.from({ length: 100 }, (_, i) => `s${i}`.repeat(i % 5));
  for (const [ty, v] of /** @type {[Type, InputValue][]} */ ([
    [t.list(t.string), strings],
    [
      t.list(t.tuple(t.string, t.u64, t.f64, t.char)),
      strings.map((s, i) => [s, BigInt(i) << 40n, i / 3, "é"]),
    ],
  ])) {
    const chunks = Array.from(encode(ty, v), (b) => Uint8Array.of(b));
    const ar = new AsyncReader(async () => chunks.shift() ?? null);
    assert.deepEqual(await ar.decode((r) => decodeSteps(r, ty)), v);
  }
});

test("a bounded channel makes senders wait for room", async () => {
  const chan = new Chan(1);
  assert.equal(await chan.send(1), true);
  let sent = false;
  const sending = chan.send(2).then((ok) => (sent = ok));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent, false);
  assert.deepEqual(await chan.next(), { value: 1, done: false });
  assert.equal(await sending, true);
  chan.close();
  assert.equal(await chan.send(3), false);
  assert.deepEqual(await chan.next(), { value: 2, done: false });
  assert.deepEqual(await chan.next(), { value: undefined, done: true });
});

test("a channel is cancelled by leaving a loop over it early, not once ended", async () => {
  for (const [
    consume,
    cancels,
  ] of /** @type {[(chan: Chan<number>) => Promise<void>, boolean][]} */ ([
    [
      async (chan) => {
        for await (const v of chan) {
          assert.equal(v, 1);
          break;
        }
      },
      true,
    ],
    [
      async (chan) => {
        chan.close();
        for await (const v of chan) assert.equal(v, 1);
        chan.cancel();
      },
      false,
    ],
    [
      async (chan) => {
        await chan.next();
        const end = chan.next();
        chan.close();
        await end;
        chan.cancel();
      },
      false,
    ],
  ])) {
    const chan = new Chan();
    let cancelled = false;
    chan.onCancel(() => (cancelled = true));
    chan.push(1);
    await consume(chan);
    assert.equal(cancelled, cancels);
    assert.equal(await chan.send(2), false);
  }
  const failed = new Chan();
  failed.close(new Error("boom"));
  failed.cancel(new Error("discarded"));
  await assert.rejects(failed.next(), /boom/);
});
