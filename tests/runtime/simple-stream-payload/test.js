import assert from "node:assert/strict";

export const handlers = () => [
  {
    async readStream(x) {
      const it = (async function* () {
        for await (const chunk of x) yield* chunk;
      })();
      const take = async (n) => {
        const out = [];
        while (out.length < n) {
          const { value, done } = await it.next();
          if (done) break;
          out.push(value);
        }
        return out;
      };
      assert.deepEqual(await take(1), [0]);
      assert.deepEqual(await take(2), [1, 2]);
      assert.deepEqual(await take(1), [3]);
      assert.deepEqual(await take(1), [4]);
    },
  },
];
