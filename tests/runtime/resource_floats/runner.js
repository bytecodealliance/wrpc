import assert from "node:assert/strict";

import { Float as Float2 } from "./imports/exports.js";
import { add } from "./runner.js";
import { Float } from "./test/resource-floats/test.js";

export async function run(connect) {
  const a = await Float.new(await connect(), 42);
  const b = await Float.new(await connect(), 55);
  let float3 = await add(await connect(), a, b);
  assert.equal(await Float.get(await connect(), float3), 114);

  float3 = await Float2.new(await connect(), 22);
  assert.equal(await Float2.get(await connect(), float3), 22 + 1 + 2 + 4 + 3);

  const res = await Float2.add(await connect(), float3, 7);
  assert.equal(await Float2.get(await connect(), res), 59);
}
