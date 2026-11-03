export const handlers = () => {
  let slot;
  return [
    {
      set(param) {
        slot = param;
      },
      get: () => slot,
    },
  ];
};
