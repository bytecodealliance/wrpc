import assert from "node:assert/strict";

import { test } from "./imports/the-test.js";
import * as a1 from "./test/resource-alias-redux/resource-alias1.js";
import * as a2 from "./test/resource-alias-redux/resource-alias2.js";

export async function run(connect) {
  const thing = async (s) => a1.Thing.new(await connect(), s);
  const get = async (t) => a1.Thing.get(await connect(), t);

  let result = await test(await connect(), [await thing("Ni Hao")]);
  assert.equal(result.length, 1);
  assert.equal(await get(result[0]), "Ni Hao GuestThing GuestThing.get");

  result = await a1.a(await connect(), { thing: await thing("Ciao") });
  assert.equal(result.length, 1);
  assert.equal(await get(result[0]), "Ciao GuestThing GuestThing.get");

  result = await a2.b(
    await connect(),
    { thing: await thing("Ciao") },
    { thing: await thing("Aloha") },
  );
  assert.equal(result.length, 2);
  assert.equal(await get(result[0]), "Ciao GuestThing GuestThing.get");
  assert.equal(await get(result[1]), "Aloha GuestThing GuestThing.get");
}
