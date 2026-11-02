import assert from "node:assert/strict";

import * as toTest from "./test/flags-widths/to-test.js";

const set = (flags) => Object.keys(flags).filter((k) => flags[k]);

export async function run(connect) {
  const [f17, b17] = await toTest.roundtripFlag17(await connect(), { b0: true, b16: true }, 7);
  assert.deepEqual(set(f17), ["b0", "b16"]);
  assert.equal(b17, 7);

  const [f40, b40] = await toTest.roundtripFlag40(await connect(), { b1: true, b39: true }, 9);
  assert.deepEqual(set(f40), ["b1", "b39"]);
  assert.equal(b40, 9);
}
