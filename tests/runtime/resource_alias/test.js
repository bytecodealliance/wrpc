export const handlers = () => [
  {
    a: (f) => [f.x],
    X: {
      new(v) {
        const h = new Uint8Array(4);
        new DataView(h.buffer).setUint32(0, v, true);
        return h;
      },
    },
  },
  { a: (f, g) => [f.x, g.x] },
];
