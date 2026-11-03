import assert from "node:assert/strict";

import { Thing } from "./test/resource-with-lists/test.js";

const bytes = (s) => new TextEncoder().encode(s);

export async function run(connect) {
  const thing = await Thing.new(await connect(), bytes("Hi"));
  assert.deepEqual(
    await Thing.foo(await connect(), thing),
    bytes("Hi Thing HostThing HostThing.foo Thing.foo"),
  );
  await Thing.bar(await connect(), thing, bytes("Hola"));
  assert.deepEqual(
    await Thing.foo(await connect(), thing),
    bytes("Hola Thing.bar HostThing.bar HostThing.foo Thing.foo"),
  );
  assert.deepEqual(
    await Thing.baz(await connect(), bytes("Ohayo Gozaimas")),
    bytes("Ohayo Gozaimas Thing.baz HostThing.baz Thing.baz again"),
  );
}
