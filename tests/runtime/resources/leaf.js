const index = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength).getUint32(0, true);

export const handlers = () => {
  const ys = [];
  const Y = {
    new(a) {
      const h = new Uint8Array(4);
      new DataView(h.buffer).setUint32(0, ys.push(a) - 1, true);
      return h;
    },
    getA: (self) => ys[index(self)],
    setA(self, a) {
      ys[index(self)] = a;
    },
    add: (y, a) => Y.new(ys[index(y)] + a),
  };
  return [{ Y }];
};
