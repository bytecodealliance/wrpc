import assert from "node:assert/strict";

import * as toTest from "./test/lists/to-test.js";

const bytes = (s) => new TextEncoder().encode(s);

export async function run(connect) {
  await toTest.emptyListParam(await connect(), new Uint8Array());
  await toTest.emptyStringParam(await connect(), "");
  assert.deepEqual(await toTest.emptyListResult(await connect()), new Uint8Array());
  assert.equal(await toTest.emptyStringResult(await connect()), "");
  await toTest.listParam(await connect(), new Uint8Array([1, 2, 3, 4]));
  await toTest.listParam2(await connect(), "foo");
  await toTest.listParam3(await connect(), ["foo", "bar", "baz"]);
  await toTest.listParam4(await connect(), [["foo", "bar"], ["baz"]]);
  await toTest.listParam5(await connect(), [
    [1, 2, 3],
    [4, 5, 6],
  ]);
  await toTest.listParamLarge(await connect(), Array(1000).fill("string"));

  assert.deepEqual(await toTest.listResult(await connect()), new Uint8Array([1, 2, 3, 4, 5]));
  assert.equal(await toTest.listResult2(await connect()), "hello!");
  assert.deepEqual(await toTest.listResult3(await connect()), ["hello,", "world!"]);
  assert.deepEqual(await toTest.listRoundtrip(await connect(), new Uint8Array()), new Uint8Array());
  assert.equal(await toTest.stringRoundtrip(await connect(), "🚀 x"), "🚀 x");

  const minmax = async (f, a, b) => assert.deepEqual(await f(await connect(), a, b), [a, b]);
  await minmax(toTest.listMinmax8, new Uint8Array([0, 0xff]), new Int8Array([-0x80, 0x7f]));
  await minmax(
    toTest.listMinmax16,
    new Uint16Array([0, 0xffff]),
    new Int16Array([-0x8000, 0x7fff]),
  );
  await minmax(
    toTest.listMinmax32,
    new Uint32Array([0, 0xffffffff]),
    new Int32Array([-0x80000000, 0x7fffffff]),
  );
  await minmax(
    toTest.listMinmax64,
    new BigUint64Array([0n, 2n ** 64n - 1n]),
    new BigInt64Array([-(2n ** 63n), 2n ** 63n - 1n]),
  );
  await minmax(
    toTest.listMinmaxFloat,
    new Float32Array([-Infinity, Infinity]),
    new Float64Array([Number.MIN_VALUE, Number.MAX_VALUE]),
  );

  const headers = [
    ["Content-Type", bytes("text/plain")],
    ["Content-Length", bytes("9")],
  ];
  assert.deepEqual(await toTest.wasiHttpHeadersRoundtrip(await connect(), headers), headers);
}
