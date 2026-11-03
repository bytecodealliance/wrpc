import * as toTest from "./test/common/to-test.js";

export const handlers = (connect) => [
  {
    wrap: async (flag) => toTest.wrap(await connect(), flag),
    varF: async () => toTest.varF(await connect()),
  },
];
