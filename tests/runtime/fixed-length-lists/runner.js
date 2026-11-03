import assert from "node:assert/strict";

import * as toTest from "./test/fixed-length-lists/to-test.js";

export async function run(connect) {
  await toTest.listParam(await connect(), Uint32Array.of(1, 2, 3, 4));
  await toTest.listParam2(await connect(), [Uint32Array.of(1, 2), Uint32Array.of(3, 4)]);
  await toTest.listParam3(
    await connect(),
    Int32Array.from({ length: 20 }, (_, i) => (i % 2 ? i + 1 : -(i + 1))),
  );

  assert.deepEqual(
    await toTest.listResult(await connect()),
    Uint8Array.of(0x30, 0x31, 0x41, 0x42, 0x61, 0x62, 128, 255),
  );

  const roundtrip = async (f, ...args) => assert.deepEqual(await f(await connect(), ...args), args);
  await roundtrip(
    toTest.listMinmax16,
    Uint16Array.of(0, 1024, 32768, 65535),
    Int16Array.of(1, 2048, -32767, -2),
  );
  await roundtrip(toTest.listMinmaxFloat, Float32Array.of(2, -42), Float64Array.of(0.25, -0.125));

  const bytes = Uint8Array.of(0x61, 0x62, 0x63, 0x64, 0, 1, 2, 3, 0x41, 0x42, 0x59, 0x5a);
  assert.deepEqual(await toTest.listRoundtrip(await connect(), bytes), bytes);

  await roundtrip(
    toTest.nestedRoundtrip,
    [Uint32Array.of(1, 5), Uint32Array.of(42, 1_000_000)],
    [Int32Array.of(-1, 3), Int32Array.of(-2_000_000, 4711)],
  );
  await roundtrip(
    toTest.largeRoundtrip,
    [Uint32Array.of(1, 5), Uint32Array.of(42, 1_000_000)],
    [
      Int32Array.of(-1, 3, -2, 4),
      Int32Array.of(-2_000_000, 4711, 99_999, -5),
      Int32Array.of(-6, 7, 8, -9),
      Int32Array.of(50, -5, 500, -5000),
    ],
  );

  const result = await toTest.nightmareOnCpp(await connect(), [
    { l: Int32Array.of(1, -1) },
    { l: Int32Array.of(2, -2) },
  ]);
  assert.deepEqual(result[0].l, Int32Array.of(1, -1));
  assert.deepEqual(result[1].l, Int32Array.of(2, -2));
}
