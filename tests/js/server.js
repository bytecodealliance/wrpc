// Serves the `sync-server` and `async-server` worlds of `tests/wit` and the
// `echo-server` world of `tests/js/wit` on the address given as the first
// argument, for the Rust clients in `tests/js.rs`, with their bindings in the
// directory given as the second argument.

import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { accept } from "../../js/src/index.js";
import { listen } from "./transport.js";

const [addr, bindings] = process.argv.slice(2);
const load = (path) => import(pathToFileURL(join(bindings, path)).href);
const [sync, async, echo] = await Promise.all([
  load("sync-server/sync-server.js"),
  load("async-server/async-server.js"),
  load("echo-server/echo-server.js"),
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

const echoHandler = {
  echo: (v) => v,
  echoAsync: (v) => v,
};

const route = (instance, name, srv) =>
  sync.handle(
    [worldHandler, syncHandler, fooHandler, resourcesHandler, strangeHandler],
    instance,
    name,
    srv,
  ) ??
  async.handle([asyncHandler], instance, name, srv) ??
  echo.handle([echoHandler], instance, name, srv) ??
  fail(`\`${instance}#${name}\` is not exported`);

listen(
  addr,
  async (lis, sock) => {
    try {
      for (let inv; (inv = await lis.accept());) {
        const [{ instance, name }, io] = inv;
        await route(instance, name, io);
      }
      await lis.io.tx.writer.close();
    } catch (err) {
      console.error("failed to serve:", err);
      sock.resetAndDestroy();
    }
  },
  async (req) => {
    const [{ instance, name }, srv] = await accept(req);
    return route(instance, name, srv);
  },
);
