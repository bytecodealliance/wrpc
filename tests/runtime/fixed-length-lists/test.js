import assert from "node:assert/strict";

const id = (a) => a;
const pair = (a, b) => [a, b];

export const handlers = () => [
  {
    listParam(a) {
      assert.deepEqual(a, Uint32Array.of(1, 2, 3, 4));
    },
    listParam2(a) {
      assert.deepEqual(a, [Uint32Array.of(1, 2), Uint32Array.of(3, 4)]);
    },
    listParam3(a) {
      assert.deepEqual(
        a,
        Int32Array.from({ length: 20 }, (_, i) => (i % 2 ? i + 1 : -(i + 1))),
      );
    },
    listMinmax16: pair,
    listMinmaxFloat: pair,
    listRoundtrip: id,
    listResult: () => Uint8Array.of(0x30, 0x31, 0x41, 0x42, 0x61, 0x62, 128, 255),
    nestedRoundtrip: pair,
    largeRoundtrip: pair,
    nightmareOnCpp: id,
  },
];
