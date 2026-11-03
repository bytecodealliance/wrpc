export const handlers = () => [
  {
    ping: async (x, y) => (await x) + y,
    pong: (x) => x,
  },
];
