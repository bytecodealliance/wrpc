import { setImmediate } from "node:timers/promises";

export const handlers = () => [
  {
    async f() {
      await setImmediate();
    },
  },
];
