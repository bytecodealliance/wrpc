import { toplevelImport } from "./intermediate.js";
import * as test from "./test/resource-import-and-export/test.js";

export const handlers = (connect) => {
  const Thing = {
    new: async (v) => test.Thing.new(await connect(), v + 1),
    foo: async (self) => (await test.Thing.foo(await connect(), self)) + 2,
    bar: async (self, v) => test.Thing.bar(await connect(), self, v + 3),
    async baz(a, b) {
      const thing = await test.Thing.baz(await connect(), a, b);
      return Thing.new((await test.Thing.foo(await connect(), thing)) + 4);
    },
  };
  return [{ toplevelExport: async (a) => toplevelImport(await connect(), a) }, { Thing }];
};
