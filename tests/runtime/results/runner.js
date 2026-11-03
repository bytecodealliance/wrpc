import assert from "node:assert/strict";

import * as test from "./test/results/test.js";

export async function run(connect) {
  const check = async (f, a, expected) =>
    assert.deepEqual(structuredClone(await f(await connect(), a)), expected);

  await check(test.stringError, 0, { kind: "error", value: "zero" });
  await check(test.stringError, 1, { kind: "ok", value: 1 });

  await check(test.enumError, 0, { kind: "error", value: "a" });
  await check(test.enumError, 1, { kind: "ok", value: 1 });

  await check(test.recordError, 0, { kind: "error", value: { line: 420, column: 0 } });
  await check(test.recordError, 1, { kind: "error", value: { line: 77, column: 2 } });
  assert.equal((await test.recordError(await connect(), 2)).kind, "ok");

  await check(test.variantError, 0, {
    kind: "error",
    value: { kind: "e2", value: { line: 420, column: 0 } },
  });
  await check(test.variantError, 1, { kind: "error", value: { kind: "e1", value: "b" } });
  await check(test.variantError, 2, { kind: "error", value: { kind: "e1", value: "c" } });

  await check(test.emptyError, 0, { kind: "error" });
  await check(test.emptyError, 1, { kind: "ok", value: 42 });
  await check(test.emptyError, 2, { kind: "ok", value: 2 });

  await check(test.doubleError, 0, { kind: "ok", value: { kind: "ok" } });
  await check(test.doubleError, 1, { kind: "ok", value: { kind: "error", value: "one" } });
  await check(test.doubleError, 2, { kind: "error", value: "two" });
}
