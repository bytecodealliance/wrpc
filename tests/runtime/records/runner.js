import assert from "node:assert/strict";

import * as toTest from "./test/records/to-test.js";

const set = (flags) => Object.keys(flags).filter((k) => flags[k]);

export async function run(connect) {
  assert.deepEqual(await toTest.multipleResults(await connect()), [4, 5]);
  assert.deepEqual(await toTest.swapTuple(await connect(), [1, 2]), [2, 1]);

  for (const f of [[], ["a"], ["b"], ["a", "b"]]) {
    const flags = Object.fromEntries(f.map((k) => [k, true]));
    assert.deepEqual(set(await toTest.roundtripFlags1(await connect(), flags)), f);
  }
  for (const f of [[], ["c"], ["d"], ["c", "e"]]) {
    const flags = Object.fromEntries(f.map((k) => [k, true]));
    assert.deepEqual(set(await toTest.roundtripFlags2(await connect(), flags)), f);
  }

  const [f8, f16, f32] = await toTest.roundtripFlags3(
    await connect(),
    { b0: true },
    { b1: true },
    { b2: true },
  );
  assert.deepEqual([set(f8), set(f16), set(f32)], [["b0"], ["b1"], ["b2"]]);

  for (const r of [
    { a: 8, b: { a: false, b: false } },
    { a: 0, b: { a: true, b: true } },
  ]) {
    assert.deepEqual(structuredClone(await toTest.roundtripRecord1(await connect(), r)), r);
  }

  assert.deepEqual(await toTest.tuple1(await connect(), [1]), [1]);
}
