import assert from "node:assert/strict";

import { Thing, test } from "./test/resource-borrow-in-record/to-test.js";

export async function run(connect) {
  const thing1 = await Thing.new(await connect(), "Bonjour");
  const thing2 = await Thing.new(await connect(), "mon cher");
  const things = await test(await connect(), [{ thing: thing1 }, { thing: thing2 }]);
  const result = [];
  for (const thing of things) result.push(await Thing.get(await connect(), thing));
  assert.deepEqual(result, ["Bonjour new test get", "mon cher new test get"]);
}
