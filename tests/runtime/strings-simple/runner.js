import assert from "node:assert/strict";

import * as cat from "./imports/cat.js";

export async function run(connect) {
  await cat.foo(await connect(), "hello");
  assert.equal(await cat.bar(await connect()), "world");
}
