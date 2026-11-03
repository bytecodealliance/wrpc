import { setImmediate } from "node:timers/promises";

export const handlers = () => [
  {
    async f() {
      for (let i = 0; i < 2; i++) await setImmediate();
    },
  },
];
