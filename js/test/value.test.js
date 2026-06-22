import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { Reader, Writer } from "../src/io.js";
import { encodeValue, decodeValue } from "../src/value.js";

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

test("integer encoding rejects out-of-range and non-integer values", () => {
  assert.throws(() => encode(t.u8, 256), /out of range/);
  assert.throws(() => encode(t.u8, -1), /out of range/);
  assert.throws(() => encode(t.s8, 128), /out of range/);
  assert.throws(() => encode(t.s8, -129), /out of range/);
  assert.throws(() => encode(t.u16, 65536), /out of range/);
  assert.throws(() => encode(t.u32, 4294967296), /out of range/);
  assert.throws(() => encode(t.u64, -1n), /out of range/);
  assert.throws(() => encode(t.u64, 1n << 64n), /out of range/);
  assert.throws(() => encode(t.u8, 1.5), /expected an integer/);
  // bounds are inclusive
  roundtrip(t.u8, 255);
  roundtrip(t.s8, -128);
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

test("char and string", () => {
  roundtrip(t.char, "a");
  roundtrip(t.char, "€");
  roundtrip(t.char, "😀");
  roundtrip(t.string, "");
  roundtrip(t.string, "hello, wRPC 👋");
});

test("byte lists round-trip as Uint8Array", () => {
  const out = roundtrip(t.list(t.u8), new Uint8Array([1, 2, 3, 255]));
  assert.ok(out instanceof Uint8Array);
  // encoding also accepts a plain array
  assert.deepEqual(decode(t.list(t.u8), encode(t.list(t.u8), [1, 2, 3])), new Uint8Array([1, 2, 3]));
});

test("lists, tuples, records", () => {
  roundtrip(t.list(t.string), ["a", "b", "c"]);
  roundtrip(t.tuple(t.u8, t.string, t.bool), [7, "x", true]);
  const rec = t.record({ keys: t.list(t.string), cursor: t.option(t.string) });
  roundtrip(rec, { keys: ["a", "b"], cursor: "next" });
  roundtrip(rec, { keys: [], cursor: undefined });
});

test("options", () => {
  const ty = t.option(t.u32);
  roundtrip(ty, 42);
  assert.equal(decode(ty, encode(ty, undefined)), undefined);
  assert.equal(decode(ty, encode(ty, null)), undefined);
});

test("results", () => {
  const ty = t.result(t.string, t.u32);
  roundtrip(ty, { tag: "ok", val: "fine" });
  roundtrip(ty, { tag: "err", val: 7 });
  // `result<_, e>`: ok carries no payload
  const unit = t.result(null, t.string);
  assert.deepEqual(decode(unit, encode(unit, { tag: "ok" })), { tag: "ok", val: undefined });
});

test("enums and variants", () => {
  const color = t.enum(["red", "green", "blue"]);
  roundtrip(color, "green");
  const err = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });
  roundtrip(err, { tag: "no-such-store" });
  roundtrip(err, { tag: "other", val: "boom" });
  // a payload-less variant case may be written as a bare string
  assert.deepEqual(decode(err, encode(err, "access-denied")), { tag: "access-denied" });
});

test("flags", () => {
  const ty = t.flags(["read", "write", "exec"]);
  roundtrip(ty, { read: true, write: false, exec: true });
  // arrays and sets are also accepted on encode
  assert.deepEqual(decode(ty, encode(ty, ["read", "exec"])), { read: true, write: false, exec: true });
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

test("the wasi:keyvalue store types", () => {
  const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });
  const open = t.result(t.own("bucket"), error);
  const get = t.result(t.option(t.list(t.u8)), error);
  const set = t.result(null, error);

  roundtrip(open, { tag: "ok", val: new Uint8Array([0xde, 0xad, 0xbe, 0xef]) });
  roundtrip(get, { tag: "ok", val: new TextEncoder().encode("value") });
  roundtrip(get, { tag: "ok", val: undefined });
  roundtrip(get, { tag: "err", val: { tag: "no-such-store" } });
  assert.deepEqual(decode(set, encode(set, { tag: "ok" })), { tag: "ok", val: undefined });
});
