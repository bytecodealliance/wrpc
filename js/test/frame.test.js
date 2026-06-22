import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { encode } from "../src/value.js";
import { encodeInvocation, decodeResults, PROTOCOL } from "../src/frame.js";
import { Writer } from "../src/io.js";

const INSTANCE = "wasi:keyvalue/store@0.2.0-draft2";

test("encodeInvocation matches the framed wire layout", () => {
  // `open("")`: protocol byte, the two names, then the root frame
  // `[path-len=0][data-len][params]` where params is the encoded `""`.
  const bytes = encodeInvocation(INSTANCE, "open", [t.string], [""]);

  const enc = new TextEncoder();
  const name = enc.encode(INSTANCE);
  const expected = Uint8Array.from([
    PROTOCOL,
    name.length, ...name,
    4, ...enc.encode("open"),
    0, // root frame path length
    1, // root frame data length
    0, // the encoded empty string (length 0)
  ]);
  assert.deepEqual(bytes, expected);
});

test("encodeInvocation rejects an argument-count mismatch", () => {
  assert.throws(() => encodeInvocation(INSTANCE, "open", [t.string], []), RangeError);
});

test("decodeResults reads a root result frame", () => {
  const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });
  const result = t.result(t.option(t.list(t.u8)), error);

  // Build the server's reply: a root frame wrapping the encoded result value.
  const value = { tag: "ok", val: new TextEncoder().encode("hello") };
  const data = encode(result, value);
  const frame = new Writer();
  frame.u8(0); // path length
  frame.varU(data.length); // data length
  frame.bytes(data);

  const [decoded] = decodeResults(frame.finish(), [result]);
  assert.deepEqual(decoded, value);
});

test("decodeResults returns [] for an empty body or no result types", () => {
  assert.deepEqual(decodeResults(new Uint8Array(), [t.string]), []);
  assert.deepEqual(decodeResults(Uint8Array.from([0, 0]), []), []);
});
