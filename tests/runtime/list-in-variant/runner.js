import assert from "node:assert/strict";

import * as toTest from "./test/list-in-variant/to-test.js";

export async function run(connect) {
  assert.equal(await toTest.listInOption(await connect(), ["hello", "world"]), "hello,world");
  assert.equal(await toTest.listInOption(await connect(), undefined), "none");

  assert.equal(
    await toTest.listInVariant(await connect(), {
      kind: "with-data",
      value: ["foo", "bar", "baz"],
    }),
    "foo,bar,baz",
  );
  assert.equal(await toTest.listInVariant(await connect(), { kind: "empty" }), "empty");

  assert.equal(
    await toTest.listInResult(await connect(), { kind: "ok", value: ["a", "b", "c"] }),
    "a,b,c",
  );
  assert.equal(
    await toTest.listInResult(await connect(), { kind: "error", value: "oops" }),
    "err:oops",
  );

  let s = await toTest.listInOptionWithReturn(await connect(), ["hello", "world"]);
  assert.equal(s.count, 2);
  assert.equal(s.label, "hello,world");
  s = await toTest.listInOptionWithReturn(await connect(), undefined);
  assert.equal(s.count, 0);
  assert.equal(s.label, "none");

  assert.equal(await toTest.topLevelList(await connect(), ["x", "y", "z"]), "x,y,z");
}
