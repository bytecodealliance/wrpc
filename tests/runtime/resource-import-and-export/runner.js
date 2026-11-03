import assert from "node:assert/strict";

import { Thing } from "./test/resource-import-and-export/test.js";

export async function run(connect) {
  const thing1 = await Thing.new(await connect(), 42);

  assert.equal(await Thing.foo(await connect(), thing1), 48);

  await Thing.bar(await connect(), thing1, 33);
  assert.equal(await Thing.foo(await connect(), thing1), 43);

  const thing2 = await Thing.new(await connect(), 81);
  const thing3 = await Thing.baz(await connect(), thing1, thing2);
  assert.equal(
    await Thing.foo(await connect(), thing3),
    33 + 3 + 3 + 81 + 1 + 1 + 2 + 2 + 4 + 1 + 2 + 4 + 1 + 1 + 2 + 2,
  );
}
