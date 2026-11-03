import assert from "node:assert/strict";

const id = (v) => v;
const pair = (a, b) => [a, b];

export const handlers = () => [
  {
    allocatedBytes() {
      return 0;
    },
    emptyListParam(a) {
      assert.deepEqual(a, new Uint8Array());
    },
    emptyStringParam(a) {
      assert.equal(a, "");
    },
    emptyListResult() {
      return new Uint8Array();
    },
    emptyStringResult() {
      return "";
    },
    listParam(a) {
      assert.deepEqual(a, new Uint8Array([1, 2, 3, 4]));
    },
    listParam2(a) {
      assert.equal(a, "foo");
    },
    listParam3(a) {
      assert.deepEqual(a, ["foo", "bar", "baz"]);
    },
    listParam4(a) {
      assert.deepEqual(a, [["foo", "bar"], ["baz"]]);
    },
    listParam5(a) {
      assert.deepEqual(a, [
        [1, 2, 3],
        [4, 5, 6],
      ]);
    },
    listParamLarge(a) {
      assert.equal(a.length, 1000);
    },
    listResult() {
      return new Uint8Array([1, 2, 3, 4, 5]);
    },
    listResult2() {
      return "hello!";
    },
    listResult3() {
      return ["hello,", "world!"];
    },
    listRoundtrip: id,
    stringRoundtrip: id,
    listMinmax8: pair,
    listMinmax16: pair,
    listMinmax32: pair,
    listMinmax64: pair,
    listMinmaxFloat: pair,
    wasiHttpHeadersRoundtrip: id,
  },
];
