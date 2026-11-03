import assert from "node:assert/strict";

import * as cat from "./imports/cat.js";

export async function run(connect) {
  await cat.foo(await connect(), new TextEncoder().encode("hello"));
  assert.deepEqual(await cat.bar(await connect()), new TextEncoder().encode("world"));
}
