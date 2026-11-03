const ok = (value) => ({ kind: "ok", value });
const error = (value) => ({ kind: "error", value });

export const handlers = () => [
  {
    stringError: (a) => (a === 0 ? error("zero") : ok(a)),
    enumError: (a) => (a === 0 ? error("a") : ok(a)),
    recordError(a) {
      if (a === 0) return error({ line: 420, column: 0 });
      if (a === 1) return error({ line: 77, column: 2 });
      return ok(a);
    },
    variantError(a) {
      if (a === 0) return error({ kind: "e2", value: { line: 420, column: 0 } });
      if (a === 1) return error({ kind: "e1", value: "b" });
      if (a === 2) return error({ kind: "e1", value: "c" });
      return ok(a);
    },
    emptyError(a) {
      if (a === 0) return { kind: "error" };
      if (a === 1) return ok(42);
      return ok(a);
    },
    doubleError(a) {
      if (a === 0) return ok({ kind: "ok" });
      if (a === 1) return ok(error("one"));
      return error("two");
    },
  },
];
