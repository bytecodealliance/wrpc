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
      assert.equal((await take(1)).length, 1);
      assert.equal((await take(2)).length, 2);
    },
  },
];
