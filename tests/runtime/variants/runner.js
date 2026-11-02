import assert from "node:assert/strict";

import * as toTest from "./test/variants/to-test.js";

const roundtrip = async (f, v, expected = v) =>
  assert.deepEqual(structuredClone(await f(v)), expected);

export async function run(connect) {
  const call =
    (f) =>
    async (...args) =>
      f(await connect(), ...args);

  await roundtrip(call(toTest.roundtripOption), 1, 1);
  await roundtrip(call(toTest.roundtripOption), undefined);
  await roundtrip(call(toTest.roundtripOption), 2, 2);

  await roundtrip(call(toTest.roundtripResult), { kind: "ok", value: 2 });
  await roundtrip(call(toTest.roundtripResult), { kind: "ok", value: 4 });
  await roundtrip(
    call(toTest.roundtripResult),
    { kind: "error", value: 5.3 },
    { kind: "error", value: 5 },
  );

  await roundtrip(call(toTest.roundtripEnum), "a");
  await roundtrip(call(toTest.roundtripEnum), "b");

  await roundtrip(call(toTest.invertBool), true, false);
  await roundtrip(call(toTest.invertBool), false, true);

  await roundtrip(call(toTest.variantCasts), [
    { kind: "a", value: 1 },
    { kind: "a", value: 2 },
    { kind: "a", value: 3 },
    { kind: "a", value: 4n },
    { kind: "a", value: 5n },
    { kind: "a", value: 6 },
  ]);
  await roundtrip(call(toTest.variantCasts), [
    { kind: "b", value: 1n },
    { kind: "b", value: 2 },
    { kind: "b", value: 3 },
    { kind: "b", value: 4 },
    { kind: "b", value: 5 },
    { kind: "b", value: 6 },
  ]);

  await roundtrip(call(toTest.variantZeros), [
    { kind: "a", value: 1 },
    { kind: "a", value: 2n },
    { kind: "a", value: 3 },
    { kind: "a", value: 4 },
  ]);
  await roundtrip(call(toTest.variantZeros), [
    { kind: "b" },
    { kind: "b" },
    { kind: "b" },
    { kind: "b" },
  ]);

  await toTest.variantTypedefs(await connect(), undefined, false, { kind: "error" });

  assert.deepEqual(
    structuredClone(await toTest.variantEnums(await connect(), true, { kind: "ok" }, "success")),
    [true, { kind: "ok" }, "success"],
  );
}
