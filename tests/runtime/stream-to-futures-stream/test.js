import assert from "node:assert/strict";

export const handlers = () => [
  {
    async readStream(x) {
      const stream = (async function* () {
        for await (const chunk of x) yield* chunk;
      })();
      assert.deepEqual(await stream.next(), { value: 10, done: false });
      assert.deepEqual(await stream.next(), { value: 20, done: false });
      assert.deepEqual(await stream.next(), { value: 30, done: false });
      assert.deepEqual(await stream.next(), { value: undefined, done: true });
    },
  },
];
