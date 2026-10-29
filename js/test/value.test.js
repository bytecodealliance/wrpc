import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

import * as t from "../src/types.js";
import { AsyncReader, Chan, Reader, Writer } from "../src/io.js";
import { encodeValue, decodeSteps, decodeValue } from "../src/value.js";

/** Encode a single value against `ty` to bytes. */
function encode(ty, v) {
  const w = new Writer();
  encodeValue(w, ty, v);
  return w.finish();
}

/** Decode a single value of type `ty` from `bytes`. */
function decode(ty, bytes) {
  return decodeValue(new Reader(bytes), ty);
}

/** A null-prototype object of `props`, as records, flags and variants decode to. */
const obj = (props) => Object.assign(Object.create(null), props);

/** Assert that `v` survives an encode/decode round-trip against `ty`. */
function roundtrip(ty, v) {
  const out = decode(ty, encode(ty, v));
  assert.deepEqual(out, v);
  return out;
}

test("booleans", () => {
  roundtrip(t.bool, true);
  roundtrip(t.bool, false);
});

test("small integers", () => {
  for (const v of [0, 1, 127, 255]) roundtrip(t.u8, v);
  for (const v of [-128, -1, 0, 127]) roundtrip(t.s8, v);
  for (const v of [0, 300, 65535]) roundtrip(t.u16, v);
  for (const v of [-32768, -1, 32767]) roundtrip(t.s16, v);
  for (const v of [0, 1, 4294967295]) roundtrip(t.u32, v);
  for (const v of [-2147483648, -1, 2147483647]) roundtrip(t.s32, v);
});

test("integers wrap like the JS-API draft converts them", () => {
  for (const [ty, v, wrapped] of [
    [t.u8, 256, 0],
    [t.u8, -1, 255],
    [t.s8, 128, -128],
    [t.s8, -129, 127],
    [t.u16, 65536, 0],
    [t.s16, 32768, -32768],
    [t.u32, 4294967296, 0],
    [t.u32, -1, 4294967295],
    [t.s32, 2147483648, -2147483648],
    [t.u64, -1n, (1n << 64n) - 1n],
    [t.u64, 1n << 64n, 0n],
    [t.s64, 1n << 63n, -(1n << 63n)],
    [t.u8, 1.5, 1],
    [t.s8, -1.5, -1],
    [t.u32, NaN, 0],
    [t.u32, Infinity, 0],
    [t.u32, "7", 7],
    [t.u32, null, 0],
    [t.u32, undefined, 0],
    [t.u64, "7", 7n],
    [t.s64, true, 1n],
  ]) {
    assert.equal(decode(ty, encode(ty, v)), wrapped, `${ty.kind} ${v}`);
  }
  for (const ty of [t.u8, t.s32, t.f64]) {
    assert.throws(() => encode(ty, 1n), TypeError);
    assert.throws(() => encode(ty, Symbol()), TypeError);
  }
  for (const ty of [t.u64, t.s64]) {
    for (const v of [1, 1.5, null, undefined]) assert.throws(() => encode(ty, v), TypeError);
  }
});

test("other scalars are converted like the JS-API draft converts them", () => {
  for (const [ty, v, converted] of [
    [t.bool, "false", true],
    [t.bool, 0, false],
    [t.bool, undefined, false],
    [t.f64, null, 0],
    [t.f64, "1.5", 1.5],
    [t.f32, 0.1, Math.fround(0.1)],
    [t.string, 1, "1"],
    [t.string, undefined, "undefined"],
    [t.string, "a\uD800", "a\uFFFD"],
    [t.string, "\uDE00\uD83D", "\uFFFD\uFFFD"],
    [t.char, 1, "1"],
    [t.enum(["red"]), ["red"], "red"],
  ]) {
    assert.equal(decode(ty, encode(ty, v)), converted, `${ty.kind} ${String(v)}`);
  }
  assert.throws(() => encode(t.string, Symbol()), TypeError);
  assert.throws(() => encode(t.enum(["red"]), "blue"), {
    name: "TypeError",
    message: "unknown enum case `blue`",
  });
});

test("integer decoding rejects out-of-range and overlong encodings", () => {
  assert.throws(() => decode(t.u32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x10)), /overflows/);
  assert.throws(
    () => decode(t.u32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x80, 0x00)),
    /overflows/,
  );
  assert.throws(() => decode(t.u16, Uint8Array.of(0x80, 0x80, 0x04)), /overflows/);
  assert.throws(() => decode(t.s8, new Uint8Array(0)), /unexpected end of input/);
  assert.throws(() => decode(t.s16, Uint8Array.of(0x80, 0x80, 0x02)), /overflows/);
  assert.throws(() => decode(t.s64, new Uint8Array(11).fill(0xff)), /overflows/);
  assert.equal(decode(t.u32, Uint8Array.of(0xff, 0xff, 0xff, 0xff, 0x0f)), 4294967295);
  assert.equal(decode(t.s32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x78)), -2147483648);
});

test("decoding rejects status bytes other than 0 and 1", () => {
  assert.throws(() => decode(t.bool, Uint8Array.of(2)), /invalid bool value byte `2`/);
  assert.throws(
    () => decode(t.option(t.u8), Uint8Array.of(2, 0)),
    /invalid option status byte `2`/,
  );
  assert.throws(() => decode(t.result(), Uint8Array.of(0xff)), /invalid result status byte `255`/);
  assert.throws(
    () => decode(t.future(t.u8), Uint8Array.of(2, 0)),
    /invalid option status byte `2`/,
  );
  assert.throws(
    () => decode(t.stream(t.u8), Uint8Array.of(2, 0)),
    /invalid option status byte `2`/,
  );
  assert.equal(decode(t.bool, Uint8Array.of(1)), true);
});

test("integers encode the same as a number and as a bigint", () => {
  const bytes = (f) => {
    const w = new Writer();
    f(w);
    return w.finish();
  };
  for (const v of [0, 1, 63, 64, 127, 128, 8191, 8192, 2 ** 31 - 1, 2 ** 31, 2 ** 32 - 1]) {
    assert.deepEqual(
      bytes((w) => w.varU(v)),
      bytes((w) => w.varU(BigInt(v))),
    );
    assert.equal(
      decode(
        t.u32,
        bytes((w) => w.varU(v)),
      ),
      v,
    );
  }
  for (const v of [0, 1, -1, 63, 64, -64, -65, 8191, 8192, -8192, -8193, 2 ** 31 - 1, -(2 ** 31)]) {
    assert.deepEqual(
      bytes((w) => w.varS(v)),
      bytes((w) => w.varS(BigInt(v))),
    );
    assert.equal(
      decode(
        t.s32,
        bytes((w) => w.varS(v)),
      ),
      v,
    );
  }
  assert.throws(() => new Writer().varU(1.5), RangeError);
});

test("64-bit integers are bigint", () => {
  roundtrip(t.u64, 0n);
  roundtrip(t.u64, 18446744073709551615n);
  roundtrip(t.s64, -9223372036854775808n);
  roundtrip(t.s64, 9223372036854775807n);
});

test("floats", () => {
  for (const v of [0, 1.5, -2.25, 3.141592653589793]) roundtrip(t.f64, v);
  roundtrip(t.f32, 1.5);
});

test("NaNs are encoded canonically", () => {
  const nan = new DataView(new ArrayBuffer(8));
  nan.setBigUint64(0, 0x7ff0000000000001n, true);
  const payload = nan.getFloat64(0, true);
  assert.deepEqual(encode(t.f64, payload), Uint8Array.of(0, 0, 0, 0, 0, 0, 0xf8, 0x7f));
  assert.deepEqual(encode(t.f32, payload), Uint8Array.of(0, 0, 0xc0, 0x7f));
  assert.ok(Number.isNaN(decode(t.f64, encode(t.f64, NaN))));
});

test("char and string", () => {
  roundtrip(t.char, "a");
  roundtrip(t.char, "€");
  roundtrip(t.char, "😀");
  roundtrip(t.string, "");
  roundtrip(t.string, "hello, wRPC 👋");
  roundtrip(t.string, "\uFEFFbom");
  roundtrip(t.list(t.char), ["\uFEFF"]);
  assert.throws(() => encode(t.char, "ab"), /single Unicode scalar/);
  assert.throws(() => encode(t.char, ""), /single Unicode scalar/);
  assert.throws(() => encode(t.char, "\uD800"), /single Unicode scalar/);
  assert.throws(() => decode(t.string, Uint8Array.of(1, 0xff)), TypeError);
  assert.throws(() => decode(t.char, Uint8Array.of(0x80, 0x80)), TypeError);
});

test("byte lists round-trip as Uint8Array", () => {
  const out = roundtrip(t.list(t.u8), new Uint8Array([1, 2, 3, 255]));
  assert.ok(out instanceof Uint8Array);
  for (const ty of [t.list(t.u8), t.own("bucket")]) {
    // encoding also accepts any iterable, converting its elements
    for (const v of [[1, 2], Uint16Array.of(1, 2), Int8Array.of(1, 2), new Set([1, 2])]) {
      assert.deepEqual(encode(ty, v), encode(ty, Uint8Array.of(1, 2)));
    }
    assert.deepEqual(encode(ty, Uint16Array.of(0x1ff)), encode(ty, Uint8Array.of(0xff)));
    // but not an `ArrayBuffer` or a `DataView`, which are not iterable
    for (const v of [Uint8Array.of(1).buffer, new DataView(new ArrayBuffer(1)), "1"]) {
      assert.throws(() => encode(ty, v), { name: "TypeError", message: /expected an iterable/ });
    }
  }
  const detached = Uint8Array.of(1);
  structuredClone(detached.buffer, { transfer: [detached.buffer] });
  assert.throws(() => encode(t.list(t.u8), detached), /attached buffer/);
});

/** Sample values of each numeric type, and the typed array lists of it map to. */
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
    const out = roundtrip(ty, typed);
    assert.ok(out instanceof Typed, `\`list<${elem.kind}>\` decodes to \`${Typed.name}\``);
    assert.ok(roundtrip(ty, new Typed(0)) instanceof Typed);
    // encoding also accepts a plain array
    assert.deepEqual(encode(ty, vals), encode(ty, typed));
    assert.deepEqual(decode(ty, encode(ty, vals)), typed);
  }
  // other iterables are converted like the typed array constructors do
  assert.deepEqual(encode(t.list(t.u16), Int32Array.of(-1)), encode(t.list(t.u16), [65535]));
  assert.deepEqual(encode(t.list(t.s8), [128]), encode(t.list(t.s8), [-128]));
  assert.deepEqual(encode(t.list(t.u64), new Set([1n])), encode(t.list(t.u64), [1n]));
  assert.throws(() => encode(t.list(t.s64), [-1, 2]), TypeError);
  assert.throws(() => encode(t.list(t.u64), Uint8Array.of(1)), TypeError);
  assert.throws(() => encode(t.list(t.u16), { length: 1 }), /expected an iterable/);
  assert.deepEqual(decode(t.list(t.list(t.u16)), encode(t.list(t.list(t.u16)), [[1], []])), [
    Uint16Array.of(1),
    new Uint16Array(0),
  ]);
});

test("typed arrays and iterables from another realm are accepted", () => {
  for (const [elem, Typed, vals] of NUMERIC) {
    const foreign = runInNewContext(`${Typed.name}.from(vals)`, { vals });
    assert.ok(!(foreign instanceof Typed));
    assert.deepEqual(encode(t.list(elem), foreign), encode(t.list(elem), Typed.from(vals)));
  }
  for (const ty of [t.list(t.u8), t.own("bucket")]) {
    for (const v of ["Uint16Array.of(1, 2)", "[1, 2]", "new Set([1, 2])"]) {
      assert.deepEqual(encode(ty, runInNewContext(v)), encode(ty, Uint8Array.of(1, 2)));
    }
    for (const v of ["Uint8Array.of(1, 2).buffer", "({ length: 1 })"]) {
      assert.throws(() => encode(ty, runInNewContext(v)), /expected an iterable/);
    }
  }
});

test("lists, tuples, records", () => {
  roundtrip(t.list(t.string), ["a", "b", "c"]);
  roundtrip(t.tuple(t.u8, t.string, t.bool), [7, "x", true]);
  const rec = t.record({ keys: t.list(t.string), cursor: t.option(t.string) });
  roundtrip(rec, obj({ keys: ["a", "b"], cursor: "next" }));
  roundtrip(rec, obj({ keys: [], cursor: undefined }));
  assert.equal(Object.getPrototypeOf(decode(rec, encode(rec, { keys: [] }))), null);
  // lists and tuples are any iterables, and missing tuple elements are `undefined`
  assert.deepEqual(decode(t.list(t.string), encode(t.list(t.string), new Set(["a"]))), ["a"]);
  const tup = t.tuple(t.u8, t.option(t.string));
  assert.deepEqual(decode(tup, encode(tup, new Set([7]))), [7, undefined]);
  assert.deepEqual(decode(tup, encode(tup, [7, "x", "extra"])), [7, "x"]);
  for (const v of ["ab", 1, { 0: 1, length: 1 }]) {
    assert.throws(() => encode(tup, v), { name: "TypeError", message: /expected an iterable/ });
    assert.throws(() => encode(t.list(t.char), v), /expected an iterable/);
  }
  assert.throws(() => encode(rec, "x"), { name: "TypeError", message: /expected an object/ });
});

test("record fields are keyed in camel case", () => {
  const ty = t.record({
    "favorite-color": t.string,
    "HTTP-error": t.u8,
    "inner-HTML": t.string,
    "max-retry-count": t.u32,
    default: t.bool,
  });
  assert.deepEqual(
    ty.fields.map(({ name, key }) => [name, key]),
    [
      ["favorite-color", "favoriteColor"],
      ["HTTP-error", "httpError"],
      ["inner-HTML", "innerHTML"],
      ["max-retry-count", "maxRetryCount"],
      ["default", "default"],
    ],
  );
  roundtrip(
    ty,
    obj({ favoriteColor: "blue", httpError: 4, innerHTML: "<p>", maxRetryCount: 3, default: true }),
  );
  assert.deepEqual(
    t.record([{ name: "a-b", ty: t.u8 }]).fields.map(({ key }) => key),
    ["aB"],
  );
});

test("names mapping to the same key are rejected", () => {
  assert.throws(() => t.record({ "a-b": t.u8, "a-B": t.u8 }), {
    name: "TypeError",
    message: "record fields `a-b` and `a-B` both have the key `aB`",
  });
  assert.throws(
    () =>
      t.record([
        { name: "a", ty: t.u8 },
        { name: "a", ty: t.u8 },
      ]),
    /record fields `a` and `a` both have the key `a`/,
  );
  assert.throws(() => t.flags(["http-only", "HTTP-only"]), {
    name: "TypeError",
    message: "flags `http-only` and `HTTP-only` both have the key `httpOnly`",
  });
});

test("names are converted to camel case like the JS-API draft converts them", () => {
  for (const [name, key] of [
    ["kebab-case", "kebabCase"],
    ["URL", "url"],
    ["XML-http-request", "xmlHttpRequest"],
    ["get-URL-path", "getURLPath"],
    ["inner-HTML", "innerHTML"],
    ["status-404", "status404"],
    ["v2-api", "v2Api"],
    ["a-b-c", "aBC"],
  ]) {
    assert.equal(t.record({ [name]: t.u8 }).fields[0].key, key, name);
    assert.equal(t.flags([name]).flags[0].key, key, name);
  }
});

test("record fields are read like properties", () => {
  const ty = t.record({ a: t.u8, b: t.option(t.u8) });
  const bytes = encode(ty, { a: 1, b: 2 });
  assert.deepEqual(encode(ty, Object.create({ a: 1, b: 2 })), bytes);
  const getters = new (class {
    get a() {
      return 1;
    }
    get b() {
      return 2;
    }
  })();
  assert.deepEqual(encode(ty, getters), bytes);
  assert.deepEqual(encode(ty, runInNewContext("({ a: 1, b: 2 })")), bytes);
  // other properties are ignored
  assert.deepEqual(encode(ty, { a: 1, b: 2, c: 3 }), bytes);
});

test("missing record fields are `undefined`", () => {
  const ty = t.record({ name: t.string, "nick-name": t.option(t.string) });
  assert.deepEqual(encode(ty, { name: "x" }), encode(ty, { name: "x", nickName: undefined }));
  assert.deepEqual(decode(ty, encode(ty, { name: "x" })), obj({ name: "x", nickName: undefined }));
  assert.deepEqual(encode(ty, { nickName: "y" }), encode(ty, { name: "undefined", nickName: "y" }));
  const nested = t.record({ "nested-option": t.option(t.option(t.u8)) });
  assert.throws(() => encode(nested, {}), {
    name: "TypeError",
    message: "expected `{ kind, value? }` for a nested `option`",
  });
  assert.deepEqual(
    decode(nested, encode(nested, { nestedOption: { kind: "none" } })),
    obj({ nestedOption: obj({ kind: "none" }) }),
  );
});

test("options", () => {
  const ty = t.option(t.u32);
  roundtrip(ty, 42);
  assert.equal(decode(ty, encode(ty, undefined)), undefined);
  assert.equal(decode(ty, encode(ty, null)), undefined);
});

test("nested options keep some(none) distinct from none", () => {
  const ty = t.option(t.option(t.u32));
  roundtrip(ty, obj({ kind: "some", value: 1 }));
  roundtrip(ty, obj({ kind: "some", value: undefined }));
  roundtrip(ty, obj({ kind: "none" }));
  assert.deepEqual(encode(ty, { kind: "some" }), Uint8Array.of(1, 0));
  assert.deepEqual(encode(ty, { kind: "none", value: 1 }), Uint8Array.of(0));
  assert.throws(() => encode(ty, undefined), /nested `option`/);
  assert.throws(() => encode(ty, { tag: "none" }), /unknown case `undefined` of a nested `option`/);
  const deep = t.option(t.option(t.option(t.string)));
  roundtrip(deep, obj({ kind: "some", value: obj({ kind: "some", value: "x" }) }));
  roundtrip(deep, obj({ kind: "some", value: obj({ kind: "none" }) }));
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
  assert.equal(Object.getPrototypeOf(decode(unit, encode(unit, { kind: "ok" }))), null);
});

test("enums and variants", () => {
  const color = t.enum(["red", "green", "blue"]);
  roundtrip(color, "green");
  const err = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });
  roundtrip(err, obj({ kind: "no-such-store" }));
  roundtrip(err, obj({ kind: "other", value: "boom" }));
  // the case is converted to a string, and the payload is read only if the case has one
  const kind = { toString: () => "other" };
  assert.deepEqual(
    encode(err, { kind, value: "boom" }),
    encode(err, { kind: "other", value: "boom" }),
  );
  assert.deepEqual(encode(err, { kind: "access-denied", value: 1 }), Uint8Array.of(1));
  assert.throws(() => encode(err, "access-denied"), {
    name: "TypeError",
    message: "expected `{ kind, value? }` for a `variant`",
  });
  assert.throws(() => encode(err, { kind: "nope" }), {
    name: "TypeError",
    message: "unknown case `nope` of a `variant`",
  });
});

test("flags", () => {
  const ty = t.flags(["read", "write", "exec"]);
  roundtrip(ty, obj({ read: true, write: false, exec: true }));
  // unset flags may be left out
  assert.deepEqual(
    decode(ty, encode(ty, { read: true, exec: true })),
    obj({ read: true, write: false, exec: true }),
  );
  // flags are converted to booleans, and other properties are ignored
  assert.deepEqual(
    encode(ty, { read: 1, write: "", exec: "yes", wirte: true }),
    Uint8Array.of(0b101),
  );
  const getter = new (class {
    get write() {
      return true;
    }
  })();
  assert.deepEqual(encode(ty, getter), Uint8Array.of(0b10));
  assert.deepEqual(encode(ty, Object.create({ exec: true })), Uint8Array.of(0b100));
  const foreign = runInNewContext("({ read: true, exec: true })");
  assert.deepEqual(encode(ty, foreign), encode(ty, { read: true, exec: true }));
  for (const v of [null, undefined, "read", 1]) {
    assert.throws(() => encode(ty, v), {
      name: "TypeError",
      message: "expected an object for `flags`",
    });
  }
  assert.throws(() => decode(ty, Uint8Array.of(0b1000)), /invalid flags bits/);
  const kebab = t.flags(["read-only", "HTTP-secure", "no-cache-at-all"]);
  assert.deepEqual(
    kebab.flags.map(({ key }) => key),
    ["readOnly", "httpSecure", "noCacheAtAll"],
  );
  roundtrip(kebab, obj({ readOnly: true, httpSecure: false, noCacheAtAll: true }));
  assert.deepEqual(encode(kebab, { httpSecure: true }), Uint8Array.of(0b10));
  assert.deepEqual(encode(kebab, { "read-only": true }), Uint8Array.of(0));
  const wide = t.flags(Array.from({ length: 9 }, (_, i) => `f${i}`));
  assert.equal(decode(wide, Uint8Array.of(0, 1)).f8, true);
  assert.throws(() => decode(wide, Uint8Array.of(0, 2)), /invalid flags bits/);
});

test("resource handles round-trip as bytes", () => {
  const handle = new Uint8Array(16).map((_, i) => i);
  const out = roundtrip(t.own("bucket"), handle);
  assert.ok(out instanceof Uint8Array);
  roundtrip(t.borrow("bucket"), handle);
});

test("stream and future cannot be encoded or decoded synchronously", () => {
  // They require the async session API; `encode`/`decode` must refuse them.
  assert.throws(() => encode(t.stream(t.u8), new Uint8Array([1])), /async session API/);
  assert.throws(() => encode(t.future(t.string), "x"), /async session API/);
  // a pending marker (`0x00`) decoded on its own is likewise rejected
  assert.throws(() => decode(t.stream(t.u8), new Uint8Array([0])), /async session API/);
  assert.throws(() => decode(t.future(t.string), new Uint8Array([0])), /async session API/);
});

test("ready streams and futures are decoded synchronously", async () => {
  const collect = async (chunks) => {
    const out = [];
    for await (const chunk of chunks) out.push(chunk);
    return out;
  };
  assert.deepEqual(await collect(decode(t.stream(t.u8), Uint8Array.of(1, 2, 7, 8))), [
    Uint8Array.of(7, 8),
  ]);
  assert.deepEqual(await collect(decode(t.stream(t.string), Uint8Array.of(1, 1, 1, 0x61))), [
    ["a"],
  ]);
  assert.deepEqual(await collect(decode(t.stream(t.u8), Uint8Array.of(1, 0))), []);
  for (const [elem, Typed, vals] of NUMERIC) {
    const w = new Writer();
    w.u8(1);
    encodeValue(w, t.list(elem), vals);
    const chunks = await collect(decode(t.stream(elem), w.finish()));
    assert.deepEqual(chunks, [Typed.from(vals)]);
    assert.ok(chunks[0] instanceof Typed);
  }
  assert.equal(await decode(t.future(t.u8), Uint8Array.of(1, 7)), 7);
  assert.equal(await decode(t.future(), Uint8Array.of(1)), undefined);
});

test("the wasi:keyvalue store types", () => {
  const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });
  const open = t.result(t.own("bucket"), error);
  const get = t.result(t.option(t.list(t.u8)), error);
  const set = t.result(null, error);

  roundtrip(open, obj({ kind: "ok", value: new Uint8Array([0xde, 0xad, 0xbe, 0xef]) }));
  roundtrip(get, obj({ kind: "ok", value: new TextEncoder().encode("value") }));
  roundtrip(get, obj({ kind: "ok", value: undefined }));
  roundtrip(get, obj({ kind: "error", value: obj({ kind: "no-such-store" }) }));
  assert.deepEqual(decode(set, encode(set, { kind: "ok" })), obj({ kind: "ok" }));
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

test("leaving a loop over a channel early cancels it", async () => {
  const chan = new Chan();
  let cancelled = false;
  chan.onCancel(() => (cancelled = true));
  chan.push(1);
  chan.push(2);
  for await (const v of chan) {
    assert.equal(v, 1);
    break;
  }
  assert.ok(cancelled);
  assert.deepEqual(await chan.next(), { value: undefined, done: true });
  assert.equal(await chan.send(3), false);
});

test("cancelling a failed channel keeps its error", async () => {
  const chan = new Chan();
  chan.close(new Error("boom"));
  chan.cancel(new Error("discarded"));
  await assert.rejects(chan.next(), /boom/);
});

test("values decode resumably across chunk boundaries", async () => {
  const strings = Array.from({ length: 100 }, (_, i) => `s${i}`.repeat(i % 5));
  for (const [ty, v] of [
    [t.list(t.string), strings],
    [
      t.list(t.tuple(t.string, t.u64, t.f64, t.char)),
      strings.map((s, i) => [s, BigInt(i) << 40n, i / 3, "é"]),
    ],
  ]) {
    const chunks = Array.from(encode(ty, v), (b) => Uint8Array.of(b));
    const ar = new AsyncReader(async () => chunks.shift() ?? null);
    assert.deepEqual(await ar.decode((r) => decodeSteps(r, ty)), v);
  }
});

test("a channel drops its cancel hooks once its end is received", async () => {
  for (const consume of [
    async (chan) => {
      chan.push(1);
      chan.close();
      for await (const v of chan) assert.equal(v, 1);
    },
    async (chan) => {
      const end = chan.next();
      chan.close();
      await end;
    },
  ]) {
    const chan = new Chan();
    let cancelled = false;
    chan.onCancel(() => (cancelled = true));
    await consume(chan);
    chan.cancel();
    assert.equal(cancelled, false);
  }
});
