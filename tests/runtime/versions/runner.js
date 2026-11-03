import assert from "node:assert/strict";

import * as v1 from "./test/dep@0.1.0/test.js";
import * as v2 from "./test/dep@0.2.0/test.js";

export async function run(connect) {
  assert.equal(await v1.x(await connect()), 1);
  assert.equal(await v1.y(await connect(), 1), 2);

  assert.equal(await v2.x(await connect()), 2);
  assert.equal(await v2.z(await connect(), 1, 1), 4);
}
