import assert from "node:assert/strict";

import { returnString } from "./my/test/i.js";

export async function run(connect) {
  assert.equal(await returnString(await connect()), "hello");
}
