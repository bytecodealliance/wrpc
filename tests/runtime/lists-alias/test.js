import assert from "node:assert/strict";

export const handlers = () => [
  {
    foo(x) {
      assert.deepEqual(x, new TextEncoder().encode("hello"));
    },
    bar: () => new TextEncoder().encode("world"),
  },
];
