export const handlers = () => [
  {
    LeafThing: {
      new: (s) => new TextEncoder().encode(s),
      get: (self) => new TextDecoder().decode(self),
    },
  },
];
