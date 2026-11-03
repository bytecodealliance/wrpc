import assert from "node:assert/strict";

export const handlers = () => [
  {
    manyArguments(...args) {
      assert.deepEqual(
        args,
        Array.from({ length: 16 }, (_, i) => BigInt(i + 1)),
      );
    },
  },
];
