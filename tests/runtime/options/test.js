import assert from "node:assert/strict";

export const handlers = () => [
  {
    optionNoneParam(a) {
      assert.equal(a, undefined);
    },
    optionSomeParam(a) {
      assert.equal(a, "foo");
    },
    optionNoneResult: () => undefined,
    optionSomeResult: () => "foo",
    optionRoundtrip: (a) => a,
    doubleOptionRoundtrip: (a) => a,
  },
];
