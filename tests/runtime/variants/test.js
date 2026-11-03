const id = (v) => v;

export const handlers = () => [
  {
    roundtripOption(a) {
      return a === undefined ? undefined : Math.trunc(a);
    },
    roundtripResult(a) {
      return a.kind === "ok" ? a : { kind: "error", value: Math.trunc(a.value) };
    },
    roundtripEnum: id,
    invertBool(a) {
      return !a;
    },
    variantCasts: id,
    variantZeros: id,
    variantTypedefs() {},
    variantEnums: (a, b, c) => [a, b, c],
  },
];
