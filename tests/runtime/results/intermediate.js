import * as test from "./test/results/test.js";

export const handlers = (connect) => [
  {
    stringError: async (a) => test.stringError(await connect(), a),
    enumError: async (a) => test.enumError(await connect(), a),
    recordError: async (a) => test.recordError(await connect(), a),
    variantError: async (a) => test.variantError(await connect(), a),
    emptyError: async (a) => test.emptyError(await connect(), a),
    doubleError: async (a) => test.doubleError(await connect(), a),
  },
];
