export const handlers = () => [
  {
    wrap: (flag) => ({ a: flag.a && !flag.b ? 1 : 2, b: flag }),
    varF: () => ({ kind: "b", value: 42 }),
  },
];
