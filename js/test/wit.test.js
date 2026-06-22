import assert from "node:assert/strict";
import { test } from "node:test";

import { parseType, parseWit } from "../src/wit.js";
import { encode, decode } from "../src/value.js";

test("parses inlined primitive and compound types", () => {
  assert.deepEqual(parseType("u64"), { kind: "u64" });
  assert.deepEqual(parseType("list<string>"), { kind: "list", elem: { kind: "string" } });
  assert.deepEqual(parseType("option<u8>"), { kind: "option", some: { kind: "u8" } });
  assert.deepEqual(parseType("result<_, string>"), {
    kind: "result",
    ok: null,
    err: { kind: "string" },
  });
  assert.deepEqual(parseType("tuple<u8, string>"), {
    kind: "tuple",
    elems: [{ kind: "u8" }, { kind: "string" }],
  });
});

test("parses records, variants, enums and flags", () => {
  assert.deepEqual(parseType("record { a: u8, b: string }"), {
    kind: "record",
    fields: [
      { name: "a", ty: { kind: "u8" } },
      { name: "b", ty: { kind: "string" } },
    ],
  });
  assert.deepEqual(parseType("variant { off, on(u8) }"), {
    kind: "variant",
    cases: [
      { name: "off", ty: null },
      { name: "on", ty: { kind: "u8" } },
    ],
  });
  assert.deepEqual(parseType("enum { red, green, blue }").names, ["red", "green", "blue"]);
  assert.deepEqual(parseType("flags { read, write }").names, ["read", "write"]);
});

test("a parsed type drives the value codec", () => {
  const ty = parseType("result<option<list<u8>>, variant { not-found, other(string) }>");
  const value = { tag: "ok", val: new Uint8Array([1, 2, 3]) };
  assert.deepEqual(decode(ty, encode(ty, value)), value);
});

test("parses a flat interface document", () => {
  const doc = parseWit(`
    package wasi:keyvalue@0.2.0-draft2;
    interface store {
      open: func(identifier: string) -> result<u32, string>;
      drop: func(handle: u32);
    }
  `);
  assert.equal(doc.package, "wasi:keyvalue@0.2.0-draft2");
  assert.equal(doc.interface, "store");
  assert.equal(doc.funcs.length, 2);
  assert.equal(doc.funcs[0].name, "open");
  assert.deepEqual(doc.funcs[0].params, [{ name: "identifier", ty: { kind: "string" } }]);
  assert.equal(doc.funcs[0].results.length, 1);
  assert.deepEqual(doc.funcs[1].results, []);
});
