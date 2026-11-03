import assert from "node:assert/strict";

let first = true;

export const handlers = () => [
  {
    fListInRecord1(a) {
      assert.equal(a.a, "list_in_record1");
    },
    fListInRecord2() {
      return { a: "list_in_record2" };
    },
    fListInRecord3(a) {
      assert.equal(a.a, "list_in_record3 input");
      return { a: "list_in_record3 output" };
    },
    fListInRecord4(a) {
      assert.equal(a.a, "input4");
      return { a: "result4" };
    },
    fListInVariant1(a, b) {
      assert.equal(a, "foo");
      assert.equal(b.kind, "error");
      assert.equal(b.value, "bar");
    },
    fListInVariant2() {
      return "list_in_variant2";
    },
    fListInVariant3(a) {
      assert.equal(a, "input3");
      return "output3";
    },
    errnoResult() {
      if (first) {
        first = false;
        return { kind: "error", value: "b" };
      }
      return { kind: "ok" };
    },
    listTypedefs(a, c) {
      assert.equal(a, "typedef1");
      assert.deepEqual(c, ["typedef2"]);
      return [new TextEncoder().encode("typedef3"), ["typedef4"]];
    },
    listOfVariants(bools, results, enums) {
      assert.deepEqual(bools, [true, false]);
      assert.deepEqual(
        results.map((r) => r.kind),
        ["ok", "error"],
      );
      assert.deepEqual(enums, ["success", "a"]);
      return [
        [false, true],
        [{ kind: "error" }, { kind: "ok" }],
        ["a", "b"],
      ];
    },
  },
];
