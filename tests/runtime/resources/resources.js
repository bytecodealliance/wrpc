import assert from "node:assert/strict";

import { Y } from "./imports/imports.js";

const view = (h) => new DataView(h.buffer, h.byteOffset, h.byteLength);
const u32 = (n) => {
  const h = new Uint8Array(4);
  view(h).setUint32(0, n, true);
  return h;
};
const i32 = (n) => {
  const h = new Uint8Array(4);
  view(h).setInt32(0, n, true);
  return h;
};

export const handlers = (connect) => {
  const xs = [];
  return [
    {
      add: (a, b) => i32(view(a).getInt32(0, true) + view(b).getInt32(0, true)),
      consume() {},
      async testImports() {
        const y = await Y.new(await connect(), 10);
        assert.equal(await Y.getA(await connect(), y), 10);
        await Y.setA(await connect(), y, 20);
        assert.equal(await Y.getA(await connect(), y), 20);
        const y2 = await Y.add(await connect(), y, 20);
        assert.equal(await Y.getA(await connect(), y2), 40);

        const y3 = await Y.new(await connect(), 1);
        const y4 = await Y.new(await connect(), 2);
        assert.equal(await Y.getA(await connect(), y3), 1);
        assert.equal(await Y.getA(await connect(), y4), 2);
        await Y.setA(await connect(), y3, 10);
        await Y.setA(await connect(), y4, 20);
        assert.equal(await Y.getA(await connect(), y3), 10);
        assert.equal(await Y.getA(await connect(), y4), 20);
        const y5 = await Y.add(await connect(), y3, 20);
        const y6 = await Y.add(await connect(), y4, 30);
        assert.equal(await Y.getA(await connect(), y5), 30);
        assert.equal(await Y.getA(await connect(), y6), 50);
        return { kind: "ok" };
      },
      X: {
        new: (a) => u32(xs.push(a) - 1),
        getA: (self) => xs[view(self).getUint32(0, true)],
        setA(self, a) {
          xs[view(self).getUint32(0, true)] = a;
        },
        add(x, a) {
          xs[view(x).getUint32(0, true)] += a;
          return x;
        },
      },
      Z: {
        new: i32,
        getA: (self) => view(self).getInt32(0, true),
        numDropped: () => 1,
      },
      KebabCase: {
        new: u32,
        getA: (self) => view(self).getUint32(0, true),
        takeOwned: (k) => view(k).getUint32(0, true),
      },
    },
  ];
};
