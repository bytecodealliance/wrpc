import assert from "node:assert/strict";

import * as toTest from "./test/flavorful/to-test.js";

export async function run(connect) {
  await toTest.fListInRecord1(await connect(), { a: "list_in_record1" });
  assert.equal((await toTest.fListInRecord2(await connect())).a, "list_in_record2");
  assert.equal(
    (await toTest.fListInRecord3(await connect(), { a: "list_in_record3 input" })).a,
    "list_in_record3 output",
  );
  assert.equal((await toTest.fListInRecord4(await connect(), { a: "input4" })).a, "result4");

  await toTest.fListInVariant1(await connect(), "foo", { kind: "error", value: "bar" });
  assert.equal(await toTest.fListInVariant2(await connect()), "list_in_variant2");
  assert.equal(await toTest.fListInVariant3(await connect(), "input3"), "output3");

  assert.equal((await toTest.errnoResult(await connect())).kind, "error");
  assert.equal((await toTest.errnoResult(await connect())).kind, "ok");

  const [a, b] = await toTest.listTypedefs(await connect(), "typedef1", ["typedef2"]);
  assert.deepEqual(a, new TextEncoder().encode("typedef3"));
  assert.deepEqual(b, ["typedef4"]);

  const [bools, results, enums] = await toTest.listOfVariants(
    await connect(),
    [true, false],
    [{ kind: "ok" }, { kind: "error" }],
    ["success", "a"],
  );
  assert.deepEqual(bools, [false, true]);
  assert.deepEqual(
    results.map((r) => r.kind),
    ["error", "ok"],
  );
  assert.deepEqual(enums, ["a", "b"]);
}
