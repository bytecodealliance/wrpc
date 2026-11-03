import assert from "node:assert/strict";

export const handlers = () => [
  {
    async f(param) {
      assert.equal(await param, 0);
    },
  },
];
