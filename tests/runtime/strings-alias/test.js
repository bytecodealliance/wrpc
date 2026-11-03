import assert from "node:assert/strict";

export const handlers = () => [
  {
    foo(x) {
      assert.equal(x, "hello");
    },
    bar: () => "world",
  },
];
