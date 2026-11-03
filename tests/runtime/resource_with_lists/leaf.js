const append = (l, s) => new Uint8Array([...l, ...new TextEncoder().encode(s)]);
const index = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength).getUint32(0, true);

export const handlers = () => {
  const things = [];
  return [
    {
      Thing: {
        new(l) {
          const h = new Uint8Array(4);
          new DataView(h.buffer).setUint32(0, things.push(append(l, " HostThing")) - 1, true);
          return h;
        },
        foo: (self) => append(things[index(self)], " HostThing.foo"),
        bar(self, l) {
          things[index(self)] = append(l, " HostThing.bar");
        },
        baz: (l) => append(l, " HostThing.baz"),
      },
    },
  ];
};
