import assert from "node:assert/strict";

import { X, Z, add, consume, testImports } from "./imports/exports.js";

export async function run(connect) {
  assert.equal((await testImports(await connect())).kind, "ok");

  const x = await X.new(await connect(), 5);
  assert.equal(await X.getA(await connect(), x), 5);
  await X.setA(await connect(), x, 10);
  assert.equal(await X.getA(await connect(), x), 10);
  const z1 = await Z.new(await connect(), 10);
  assert.equal(await Z.getA(await connect(), z1), 10);
  const z2 = await Z.new(await connect(), 20);
  assert.equal(await Z.getA(await connect(), z2), 20);

  const xadd = await X.add(await connect(), x, 5);
  assert.equal(await X.getA(await connect(), xadd), 15);

  const zadd = await add(await connect(), z1, z2);
  assert.equal(await Z.getA(await connect(), zadd), 30);

  await Z.numDropped(await connect());
  await consume(await connect(), xadd);
}
