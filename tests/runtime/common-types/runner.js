import assert from "node:assert/strict";

import * as toTest from "./test/common/to-test.js";

export async function run(connect) {
  const res = await toTest.wrap(await connect(), { a: true });
  assert.deepEqual(structuredClone(res), { a: 1, b: { a: true, b: false } });

  const res2 = await toTest.wrap(await connect(), { b: true });
  assert.deepEqual(structuredClone(res2), { a: 2, b: { a: false, b: true } });

  const res3 = await toTest.varF(await connect());
  assert.deepEqual(structuredClone(res3), { kind: "b", value: 42 });
}
