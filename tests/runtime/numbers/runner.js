import assert from "node:assert/strict";

import * as numbers from "./test/numbers/numbers.js";

export async function run(connect) {
  const roundtrip = async (f, values) => {
    for (const v of values) assert.equal(await f(await connect(), v), v);
  };
  await roundtrip(numbers.roundtripU8, [1, 0, 0xff]);
  await roundtrip(numbers.roundtripS8, [1, -0x80, 0x7f]);
  await roundtrip(numbers.roundtripU16, [1, 0, 0xffff]);
  await roundtrip(numbers.roundtripS16, [1, -0x8000, 0x7fff]);
  await roundtrip(numbers.roundtripU32, [1, 0, 0xffffffff]);
  await roundtrip(numbers.roundtripS32, [1, -0x80000000, 0x7fffffff]);
  await roundtrip(numbers.roundtripU64, [1n, 0n, 2n ** 64n - 1n]);
  await roundtrip(numbers.roundtripS64, [1n, -(2n ** 63n), 2n ** 63n - 1n]);
  await roundtrip(numbers.roundtripF32, [1, Infinity, -Infinity, NaN]);
  await roundtrip(numbers.roundtripF64, [1, Infinity, -Infinity, NaN]);
  await roundtrip(numbers.roundtripChar, ["a", " ", "🚩"]);

  await numbers.setScalar(await connect(), 2);
  assert.equal(await numbers.getScalar(await connect()), 2);
  await numbers.setScalar(await connect(), 4);
  assert.equal(await numbers.getScalar(await connect()), 4);
}
