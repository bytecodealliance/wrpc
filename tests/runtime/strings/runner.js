import assert from "node:assert/strict";

import * as toTest from "./test/strings/to-test.js";

export async function run(connect) {
  await toTest.takeBasic(await connect(), "latin utf16");
  assert.equal(await toTest.returnUnicode(await connect()), "🚀🚀🚀 𠈄𓀀");
  assert.equal(await toTest.returnEmpty(await connect()), "");
  assert.equal(await toTest.roundtrip(await connect(), "🚀🚀🚀 𠈄𓀀"), "🚀🚀🚀 𠈄𓀀");
}
