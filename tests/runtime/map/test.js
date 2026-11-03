import assert from "node:assert/strict";

const id = (a) => a;
const invert = (a) => new Map([...a].map(([k, v]) => [v, k]));

export const handlers = () => [
  {
    namedRoundtrip(a) {
      assert.equal(a.get(1), "uno");
      assert.equal(a.get(2), "two");
      return invert(a);
    },
    bytesRoundtrip(a) {
      assert.deepEqual(a.get("hello"), new TextEncoder().encode("world"));
      assert.deepEqual(a.get("bin"), Uint8Array.of(0, 1, 2));
      return a;
    },
    emptyRoundtrip(a) {
      assert.equal(a.size, 0);
      return a;
    },
    optionRoundtrip(a) {
      assert.equal(a.get("some"), 42);
      assert.ok(a.has("none") && a.get("none") === undefined);
      return a;
    },
    recordRoundtrip(a) {
      assert.equal(a.label, "test-label");
      assert.equal(a.values.size, 2);
      assert.equal(a.values.get(10), "ten");
      assert.equal(a.values.get(20), "twenty");
      return a;
    },
    inlineRoundtrip: invert,
    largeRoundtrip: id,
    multiParamRoundtrip(a, b) {
      assert.equal(a.size, 2);
      assert.equal(b.size, 1);
      return [invert(a), b];
    },
    nestedRoundtrip(a) {
      assert.equal(a.size, 2);
      assert.equal(a.get("group-a").get(1), "one");
      assert.equal(a.get("group-a").get(2), "two");
      assert.equal(a.get("group-b").get(10), "ten");
      return a;
    },
    variantRoundtrip: id,
    resultRoundtrip: id,
    tupleRoundtrip(a) {
      assert.equal(a[0].size, 1);
      assert.equal(a[0].get(7), "seven");
      assert.equal(a[1], 42n);
      return a;
    },
    singleEntryRoundtrip(a) {
      assert.equal(a.size, 1);
      return a;
    },
  },
];
