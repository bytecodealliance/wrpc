import { Thing } from "./test/resource-with-lists/test.js";

const append = (l, s) => new Uint8Array([...l, ...new TextEncoder().encode(s)]);

export const handlers = (connect) => [
  {
    Thing: {
      new: async (l) => Thing.new(await connect(), append(l, " Thing")),
      foo: async (self) => append(await Thing.foo(await connect(), self), " Thing.foo"),
      bar: async (self, l) => Thing.bar(await connect(), self, append(l, " Thing.bar")),
      baz: async (l) =>
        append(await Thing.baz(await connect(), append(l, " Thing.baz")), " Thing.baz again"),
    },
  },
];
