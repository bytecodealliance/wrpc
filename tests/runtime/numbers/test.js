let scalar = 0;

const id = (v) => v;

export const handlers = () => [
  {
    roundtripU8: id,
    roundtripS8: id,
    roundtripU16: id,
    roundtripS16: id,
    roundtripU32: id,
    roundtripS32: id,
    roundtripU64: id,
    roundtripS64: id,
    roundtripF32: id,
    roundtripF64: id,
    roundtripChar: id,
    setScalar(v) {
      scalar = v;
    },
    getScalar() {
      return scalar;
    },
  },
];
