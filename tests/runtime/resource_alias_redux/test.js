export const handlers = () => [
  {
    a: (f) => [f.thing],
    Thing: {
      new: (s) => new TextEncoder().encode(`${s} GuestThing`),
      get: (self) => `${new TextDecoder().decode(self)} GuestThing.get`,
    },
  },
  { b: (f, g) => [f.thing, g.thing] },
  { test: (things) => things },
];
