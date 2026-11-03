export const handlers = () => [
  {
    async readFuture(x) {
      await x;
    },
    dropFuture() {},
  },
];
