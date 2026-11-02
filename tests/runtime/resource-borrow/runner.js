import assert from "node:assert/strict";

import * as toTest from "./test/resource-borrow/to-test.js";

export async function run(connect) {
  const thing = await toTest.Thing.new(await connect(), 254);
  assert.equal(await toTest.foo(await connect(), thing), 254 + 1 + 2);
}
