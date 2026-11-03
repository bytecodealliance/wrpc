const handle = (s) => new TextEncoder().encode(s);
const contents = (h) => new TextDecoder().decode(h);

export const handlers = () => [
  {
    test: (a) => a.map((foo) => handle(`${contents(foo.thing)} test`)),
    Thing: {
      new: (s) => handle(`${s} new`),
      get: (self) => `${contents(self)} get`,
    },
  },
];
