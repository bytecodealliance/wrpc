import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";

import * as i from "./my/test/i.js";

const pending = Symbol("pending");

export async function run(connect) {
  {
    const { promise, resolve } = Promise.withResolvers();
    const call = i.pendingImport(await connect(), promise);
    assert.equal(await Promise.race([call, pending]), pending);
    assert.equal(await Promise.race([call, pending]), pending);
    resolve();
    const { finished } = await call;
    await finished;
  }

  {
    const { promise, resolve } = Promise.withResolvers();
    const call = i.pendingImport(await connect(), promise);
    assert.equal(await Promise.race([call, pending]), pending);
    resolve();
    for (let n = 0; n < 5; n++) await setImmediate();
  }
}
