export const handlers = () => [
  {
    foo(v) {
      return new DataView(v.buffer, v.byteOffset, v.byteLength).getUint32(0, true) + 2;
    },
    Thing: {
      new(v) {
        const handle = new Uint8Array(4);
        new DataView(handle.buffer).setUint32(0, v + 1, true);
        return handle;
      },
    },
  },
];
