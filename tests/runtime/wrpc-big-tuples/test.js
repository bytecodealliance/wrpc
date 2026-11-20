import assert from "node:assert/strict";

export const handlers = () => [
  {
    roundtrip: (a) => a,
    nestedFuture: (a) => a[16],
    manyParams(...args) {
      assert.deepEqual(
        args.slice(0, 16),
        Array.from({ length: 16 }, (_, i) => i),
      );
      return args[16];
    },
  },
];
