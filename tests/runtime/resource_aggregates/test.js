const val = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength).getUint32(0, true);

export const handlers = () => [
  {
    foo(r1, r2, r3, t1, t2, v1, v2, l1, l2, o1, o2, result1, result2) {
      return (
        [
          r1.thing,
          r2.thing,
          r3.thing1,
          r3.thing2,
          t1[0],
          t1[1].thing,
          t2[0],
          v1.value,
          v2.value,
          ...l1,
          ...l2,
          o1,
          o2,
          result1.value,
          result2.value,
        ].reduce((sum, v) => sum + (v ? val(v) : 0), 0) + 3
      );
    },
    Thing: {
      new(v) {
        const h = new Uint8Array(4);
        new DataView(h.buffer).setUint32(0, v + 1, true);
        return h;
      },
    },
  },
];
