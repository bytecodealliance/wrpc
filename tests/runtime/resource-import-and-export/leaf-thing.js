const index = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength).getUint32(0, true);

export const handlers = () => {
  const things = [];
  const Thing = {
    new(v) {
      const h = new Uint8Array(4);
      new DataView(h.buffer).setUint32(0, things.push(v + 1) - 1, true);
      return h;
    },
    foo: (self) => things[index(self)] + 2,
    bar(self, v) {
      things[index(self)] = v + 3;
    },
    baz: (a, b) => Thing.new(things[index(a)] + 2 + things[index(b)] + 2 + 4),
  };
  return [{ Thing }];
};
