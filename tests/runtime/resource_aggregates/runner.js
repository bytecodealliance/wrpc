import assert from "node:assert/strict";

import { Thing, foo } from "./test/resource-aggregates/to-test.js";

export async function run(connect) {
  const thing = async (v) => Thing.new(await connect(), v);
  const result = await foo(
    await connect(),
    { thing: await thing(0) },
    { thing: await thing(1) },
    { thing1: await thing(2), thing2: await thing(3) },
    [await thing(4), { thing: await thing(5) }],
    [await thing(6)],
    { kind: "thing", value: await thing(7) },
    { kind: "thing", value: await thing(8) },
    [await thing(9), await thing(10)],
    [await thing(11), await thing(12)],
    await thing(13),
    await thing(14),
    { kind: "ok", value: await thing(15) },
    { kind: "ok", value: await thing(16) },
  );
  assert.equal(result, ((1 + 17) * 17) / 2 + 3);
}
