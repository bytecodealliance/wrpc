import assert from "node:assert/strict";

export const handlers = () => [
  {
    cancelBeforeRead() {},
    async cancelAfterRead(x) {
      await Promise.race([x, undefined]);
    },
    async startReadThenCancel(data, signal) {
      await Promise.race([data, undefined]);
      await signal;
      assert.equal(await data, 4);
    },
  },
];
