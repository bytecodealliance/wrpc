import assert from "node:assert/strict";

import * as i from "./a/b/i.js";

export async function run(connect) {
  await i.oneArgument(await connect(), 1);
  assert.equal(await i.oneResult(await connect()), 2);
  assert.equal(await i.oneArgumentAndResult(await connect(), 3), 4);
  await i.twoArguments(await connect(), 5, 6);
  assert.equal(await i.twoArgumentsAndResult(await connect(), 7, 8), 9);
}
