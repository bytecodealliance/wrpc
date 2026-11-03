import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";

import * as i from "./my/test/i.js";

const pending = Symbol("pending");

export async function run(connect) {
  const pendingImport = async (x) => {
    const transport = await connect();
    const call = i.pendingImport(transport, x);
    const cancel = () => {
      call.then(({ finished }) => finished).catch(() => {});
      return transport.abort(new Error("cancelled"));
    };
    return { call, cancel };
  };
  const assertPending = async ({ call }) =>
    assert.equal(await Promise.race([call, pending]), pending);

  {
    const imp = await pendingImport(new Promise(() => {}));
    await assertPending(imp);
    await imp.cancel();
  }

  {
    const imp = await pendingImport(new Promise(() => {}));
    await imp.cancel();
  }

  {
    const started = await pendingImport(new Promise(() => {}));
    await assertPending(started);

    await i.backpressureSet(await connect(), true);
    const starting = await pendingImport(new Promise(() => {}));
    await assertPending(starting);

    await starting.cancel();
    await started.cancel();

    await i.backpressureSet(await connect(), false);
  }

  {
    const { promise, resolve } = Promise.withResolvers();
    const imp = await pendingImport(promise);
    await assertPending(imp);

    resolve();
    for (let n = 0; n < 5; n++) await setImmediate();

    await imp.cancel();
  }

  {
    const { promise, resolve } = Promise.withResolvers();
    const started = await pendingImport(promise);
    await assertPending(started);

    await i.backpressureSet(await connect(), true);
    const starting = await pendingImport(new Promise(() => {}));
    await assertPending(starting);

    await i.backpressureSet(await connect(), false);
    for (let n = 0; n < 5; n++) await setImmediate();

    await starting.cancel();

    resolve();
    const { finished } = await started.call;
    await finished;
  }
}
