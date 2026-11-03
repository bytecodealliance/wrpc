import { setImmediate } from "node:timers/promises";

import * as iMiddle from "./test/common/i-middle.js";

export const handlers = (connect) => [
  {
    async f() {
      let finished = false;
      const task = (async () => iMiddle.f(await connect()))().finally(() => (finished = true));
      while (!finished) await setImmediate();
      await task;
    },
  },
];
