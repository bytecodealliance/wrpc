import assert from "node:assert/strict";

export const handlers = () => [
  {
    async readStream(x) {
      let n = 0;
      for await (const chunk of x) n += chunk;
      assert.equal(n, 3);
    },
  },
];
