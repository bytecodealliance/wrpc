const id = (v) => v;

export const handlers = () => [
  {
    multipleResults() {
      return [4, 5];
    },
    swapTuple([a, b]) {
      return [b, a];
    },
    roundtripFlags1: id,
    roundtripFlags2: id,
    roundtripFlags3: (a, b, c) => [a, b, c],
    roundtripRecord1: id,
    tuple1: id,
  },
];
