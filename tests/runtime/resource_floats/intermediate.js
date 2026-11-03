import { Float as ImportFloat1 } from "./imports/imports.js";
import { Float as ImportFloat2 } from "./test/resource-floats/test.js";

export const handlers = (connect) => {
  const Float = {
    new: async (v) => ImportFloat1.new(await connect(), v + 1),
    get: async (self) => (await ImportFloat1.get(await connect(), self)) + 3,
    async add(a, b) {
      const c = await ImportFloat1.add(await connect(), a, b);
      return Float.new((await ImportFloat1.get(await connect(), c)) + 5);
    },
  };
  return [
    {
      async add(a, b) {
        const x = await ImportFloat2.get(await connect(), a);
        const y = await ImportFloat2.get(await connect(), b);
        return ImportFloat2.new(await connect(), x + y + 5);
      },
    },
    { Float },
  ];
};
