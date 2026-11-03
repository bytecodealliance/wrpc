const float = (v) => {
  const h = new Uint8Array(8);
  new DataView(h.buffer).setFloat64(0, v, true);
  return h;
};
const val = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength).getFloat64(0, true);

export const handlers = () => {
  const Float = {
    new: (v) => float(v + 2),
    get: (self) => val(self) + 4,
    add: (a, b) => Float.new(val(a) + b + 6),
  };
  return [{ Float: { new: (v) => float(v + 1), get: (self) => val(self) + 3 } }, { Float }];
};
