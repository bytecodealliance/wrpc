// Serves the `sync-server` and `async-server` worlds of `tests/wit` on the
// address given as the first argument, for the Rust clients in `tests/js.rs`,
// with their bindings in the directory given as the second argument.

import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { accept } from "../../js/src/index.js";
import { listen } from "./transport.js";

const [addr, bindings] = process.argv.slice(2);
const load = (path) => import(pathToFileURL(join(bindings, path)).href);
const [world, foo, sync, resources, strange, async] = await Promise.all([
  load("sync-server/sync-server.js"),
  load("sync-server/exports/foo.js"),
  load("sync-server/exports/wrpc-test/integration/sync.js"),
  load("sync-server/exports/wrpc-test/integration/resources.js"),
  load("sync-server/exports/strange.js"),
  load("async-server/exports/wrpc-test/integration/async.js"),
]);

const r = (foo) => ({ nested: { foo } });

const handle = (v) => new TextDecoder().decode(v);
const fooHandle = new TextEncoder().encode("foo");
const withFoo = (v, ret) => (handle(v) === "foo" ? ret : fail("unknown `foo`"));

const counters = [];
const newCounter = (n) => new TextEncoder().encode(`${counters.push(n) - 1}`);
const counter = (v) => {
  const id = handle(v);
  const i = Number(id);
  return `${i}` === id && i in counters ? i : fail("unknown `counter`");
};

function fail(msg) {
  throw new Error(msg);
}

const worldHandler = {
  ping: () => "pong",
};

const fooHandler = {
  f: (x) => (x === "f" ? 42 : fail(`expected \`f\`, got \`${x}\``)),
  foo: (x) => (x === "foo" ? undefined : fail(`expected \`foo\`, got \`${x}\``)),
};

const syncHandler = {
  fallible: (ok) => (ok ? { kind: "ok", value: true } : { kind: "error", value: "test" }),
  numbers: () => [1, 2, 3, 4n, 5, 6, 7, 8n, 9, 10],
  withFlags: (a, b, c) => ({ a, b, c }),
  withVariantOption: (ok) => (ok ? { kind: "var", value: r("bar") } : undefined),
  withVariantList: () => [
    { kind: "empty" },
    { kind: "var", value: r("foo") },
    { kind: "empty" },
    { kind: "empty" },
    { kind: "empty" },
    { kind: "var", value: r("bar") },
  ],
  withRecord: () => r("foo"),
  withRecordList: (n) => Array.from({ length: n }, (_, i) => r(`${i}`)),
  withRecordTuple: () => [r("0"), r("1")],
  withEnum: () => "bar",
  identityChar: (c) => c,
  identityOptionLists: (a, b) => [a, b],
};

const resourcesHandler = {
  bar: (v) => withFoo(v, "bar"),
  Foo: {
    new: () => fooHandle,
    foo: (v) => withFoo(v, "foo"),
    bar: (v) => withFoo(v, "bar"),
  },
  Counter: {
    new: (n) => newCounter(n),
    cloneCounter: (v) => newCounter(counters[counter(v)]),
    getCount: (v) => counters[counter(v)],
    incrementBy: (v, n) => {
      const i = counter(v);
      counters[i] = (counters[i] + n) >>> 0;
    },
    sum: (a, b) => (counters[counter(a)] + counters[counter(b)]) >>> 0,
  },
};

const strangeHandler = {
  bar: (v) => withFoo(v, 42n),
};

const asyncHandler = {
  withStreams: () => [new TextEncoder().encode("test"), [[["foo"]], [["bar", "baz"]]]],
  withFuture: (x, s) => (x.foo === "bar" ? s : fail(`expected \`bar\`, got \`${x.foo}\``)),
  identityNestedAsync: (v) => v,
  identityFuture: (v) => v,
  identityCases: (o, r, v) => [o, r, v],
};

const instances = {
  "": (inv) => world.serveInterface(inv, worldHandler),
  "wrpc-test:integration/foo": (inv) => foo.serveInterface(inv, fooHandler),
  "wrpc-test:integration/sync": (inv) => sync.serveInterface(inv, syncHandler),
  "wrpc-test:integration/resources": (inv) => resources.serveInterface(inv, resourcesHandler),
  "wrpc-test:integration/strange": (inv) => strange.serveInterface(inv, strangeHandler),
  "wrpc-test:integration/async": (inv) => async.serveInterface(inv, asyncHandler),
};

listen(addr, async (transport) => {
  let inv;
  try {
    inv = await accept(transport);
    const serve = instances[inv.instance] ?? fail(`unknown instance \`${inv.instance}\``);
    await serve(inv);
    await inv.finished;
  } catch (err) {
    inv?.mux.abort(err);
    console.error(`failed to serve \`${inv?.instance}.${inv?.func}\`:`, err);
  }
});
