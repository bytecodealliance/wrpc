import assert from "node:assert/strict";

export const handler = {
  takeBasic(s) {
    assert.equal(s, "latin utf16");
  },
  returnUnicode() {
    return "🚀🚀🚀 𠈄𓀀";
  },
  returnEmpty() {
    return "";
  },
  roundtrip: (s) => s,
};
