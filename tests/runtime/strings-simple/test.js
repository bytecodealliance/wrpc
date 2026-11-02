import assert from "node:assert/strict";

export const handler = {
  foo(x) {
    assert.equal(x, "hello");
  },
  bar() {
    return "world";
  },
};
