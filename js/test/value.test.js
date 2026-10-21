import assert from "node:assert/strict";
import { test } from "node:test";

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

test("encoding rejects values of the wrong JavaScript type", () => {
  assert.throws(() => encode(t.u32, null), /expected an integer/);
  assert.throws(() => encode(t.u32, "1"), /expected an integer/);
  assert.throws(() => encode(t.f64, null), /expected a number/);
  assert.throws(() => encode(t.string, undefined), /expected a string/);
  assert.throws(() => encode(t.bool, undefined), /expected a boolean/);
  assert.throws(() => encode(t.bool, "false"), /expected a boolean/);
  assert.throws(() => encode(t.enum(["red"]), ["red"]), /unknown enum case/);
  assert.throws(() => encode(t.string, 1), /expected a string/);
  assert.throws(() => encode(t.list(t.u8), [256]), /out of range/);
  // eslint-disable-next-line no-sparse-arrays
  assert.throws(() => encode(t.list(t.string), ["a", , "c"]), /expected a string/);
});

test("integer decoding rejects out-of-range and overlong encodings", () => {
  assert.throws(() => decode(t.u32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x10)), /overflows/);
  assert.throws(() => decode(t.u32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x80, 0x00)), /overflows/);
  assert.throws(() => decode(t.u16, Uint8Array.of(0x80, 0x80, 0x04)), /overflows/);
  assert.throws(() => decode(t.s8, new Uint8Array(0)), /unexpected end of input/);
  assert.throws(() => decode(t.s16, Uint8Array.of(0x80, 0x80, 0x02)), /overflows/);
  assert.throws(() => decode(t.s64, new Uint8Array(11).fill(0xff)), /overflows/);
  assert.equal(decode(t.u32, Uint8Array.of(0xff, 0xff, 0xff, 0xff, 0x0f)), 4294967295);
  assert.equal(decode(t.s32, Uint8Array.of(0x80, 0x80, 0x80, 0x80, 0x78)), -2147483648);
});

test("decoding rejects status bytes other than 0 and 1", () => {
  assert.throws(() => decode(t.bool, Uint8Array.of(2)), /invalid bool value byte `2`/);
  assert.throws(() => decode(t.option(t.u8), Uint8Array.of(2, 0)), /invalid option status byte `2`/);
  assert.throws(() => decode(t.result(), Uint8Array.of(0xff)), /invalid result status byte `255`/);
  assert.throws(() => decode(t.future(t.u8), Uint8Array.of(2, 0)), /invalid option status byte `2`/);
  assert.equal(decode(t.bool, Uint8Array.of(1)), true);
});

test("integers encode the same as a number and as a bigint", () => {
  const bytes = (f) => {
    const w = new Writer();
    f(w);
    return w.finish();
  };
  for (const v of [0, 1, 63, 64, 127, 128, 8191, 8192, 2 ** 31 - 1, 2 ** 31, 2 ** 32 - 1]) {
    assert.deepEqual(bytes((w) => w.varU(v)), bytes((w) => w.varU(BigInt(v))));
    assert.equal(decode(t.u32, bytes((w) => w.varU(v))), v);
  }
  for (const v of [0, 1, -1, 63, 64, -64, -65, 8191, 8192, -8192, -8193, 2 ** 31 - 1, -(2 ** 31)]) {
    assert.deepEqual(bytes((w) => w.varS(v)), bytes((w) => w.varS(BigInt(v))));
    assert.equal(decode(t.s32, bytes((w) => w.varS(v))), v);
  }
  assert.deepEqual(encode(t.u32, 300n), encode(t.u32, 300));
  assert.throws(() => encode(t.u32, 1n << 70n), /out of range/);
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
  assert.throws(() => encode(t.string, "a\uD800"), /lone surrogates/);
  assert.throws(() => encode(t.string, "\uDE00\uD83D"), /lone surrogates/);
  assert.throws(() => decode(t.string, Uint8Array.of(1, 0xff)), TypeError);
  assert.throws(() => decode(t.char, Uint8Array.of(0x80, 0x80)), TypeError);
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

test("nested options keep some(none) distinct from none", () => {
  const ty = t.option(t.option(t.u32));
  roundtrip(ty, { tag: "some", val: 1 });
  roundtrip(ty, { tag: "some", val: undefined });
  roundtrip(ty, { tag: "none" });
  assert.deepEqual(encode(ty, { tag: "some", val: undefined }), Uint8Array.of(1, 0));
  assert.deepEqual(encode(ty, { tag: "none" }), Uint8Array.of(0));
  assert.throws(() => encode(ty, undefined), /nested `option`/);
  const deep = t.option(t.option(t.option(t.string)));
  roundtrip(deep, { tag: "some", val: { tag: "some", val: "x" } });
  roundtrip(deep, { tag: "some", val: { tag: "none" } });
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
  assert.throws(() => encode(ty, { read: true, wirte: true }), /unknown flag `wirte`/);
  assert.throws(() => encode(ty, new Set(["exe"])), /unknown flag `exe`/);
  assert.throws(() => decode(ty, Uint8Array.of(0b1000)), /invalid flags bits/);
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
