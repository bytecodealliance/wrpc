import assert from "node:assert/strict";

export const handlers = () => [
  {
    oneArgument(x) {
      assert.equal(x, 1);
    },
    oneResult: () => 2,
    oneArgumentAndResult(x) {
      assert.equal(x, 3);
      return 4;
    },
    twoArguments(x, y) {
      assert.equal(x, 5);
      assert.equal(y, 6);
    },
    twoArgumentsAndResult(x, y) {
      assert.equal(x, 7);
      assert.equal(y, 8);
      return 9;
    },
  },
];
