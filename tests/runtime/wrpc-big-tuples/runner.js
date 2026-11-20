import assert from "node:assert/strict";

import * as toTest from "./wrpc-test/big-tuples/to-test.js";

export async function run(connect) {
  const a = [...Array.from({ length: 16 }, (_, i) => i), "foo"];
  assert.deepEqual(await toTest.roundtrip(await connect(), [a]), [a]);

  const { promise, resolve } = Promise.withResolvers();
  const elements = [...Array.from({ length: 16 }, (_, i) => i), promise];
  const [{ result, finished }] = await Promise.all([
    toTest.nestedFuture(await connect(), elements),
    resolve(16),
  ]);
  const [v] = await Promise.all([result, finished]);
  assert.equal(v, 16);

  assert.equal(await toTest.manyParams(await connect(), ...a), "foo");
}
