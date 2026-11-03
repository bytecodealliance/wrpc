import assert from "node:assert/strict";

import * as component from "./foo/bar/component.js";

export async function run(connect) {
  assert.ok(component.UnusedEnum && component.UnusedRecord && component.UnusedVariant);
  await component.foo(await connect());
}
