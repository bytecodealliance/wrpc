import assert from "node:assert/strict";

import * as toTest from "./test/options/to-test.js";

export async function run(connect) {
  await toTest.optionNoneParam(await connect(), undefined);
  await toTest.optionSomeParam(await connect(), "foo");
  assert.equal(await toTest.optionNoneResult(await connect()), undefined);
  assert.equal(await toTest.optionSomeResult(await connect()), "foo");
  assert.equal(await toTest.optionRoundtrip(await connect(), "foo"), "foo");
  for (const v of [
    { kind: "some", value: 42 },
    { kind: "some", value: undefined },
    { kind: "none" },
  ]) {
    assert.deepEqual(structuredClone(await toTest.doubleOptionRoundtrip(await connect(), v)), v);
  }
}
