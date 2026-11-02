// Serves the `sync-server` and `async-server` worlds of `tests/wit` on the
// address given as the first argument, for the Rust clients in `tests/js.rs`.

import { accept, t } from "../../js/src/index.js";
import { listen } from "./transport.js";

/**
 * @typedef {import("../../js/src/index.js").InputValue} InputValue
 * @typedef {import("../../js/src/index.js").Type} Type
 * @typedef {import("../../js/src/index.js").Value} Value
 */

const rec = t.record({ nested: t.record({ foo: t.string }) });
const v = t.variant({ var: rec, empty: null });
const cases = [
  t.option(t.future()),
  t.result(t.stream(t.u8), t.future(t.u32)),
  t.variant({
    empty: null,
    fut: t.future(t.string),
    elements: t.list(t.future(t.string)),
    pair: t.tuple(t.u8, t.future(t.string)),
    chunks: t.stream(t.future(t.string)),
  }),
];
/** @param {string} foo */
const r = (foo) => ({ nested: { foo } });

/** @param {Value} v */
const handle = (v) => new TextDecoder().decode(/** @type {Uint8Array} */ (v));
const fooHandle = new TextEncoder().encode("foo");
/** @template T @param {Value} v @param {T} ret @returns {T} */
const withFoo = (v, ret) => (handle(v) === "foo" ? ret : fail("unknown `foo`"));

/** @type {number[]} */
const counters = [];
/** @param {number} n */
const newCounter = (n) => new TextEncoder().encode(`${counters.push(n) - 1}`);
/** @param {Value} v */
const counter = (v) => {
  const id = handle(v);
  const i = Number(id);
  return `${i}` === id && i in counters ? i : fail("unknown `counter`");
};

/**
 * The parameter and result types and the implementation of each function, by
 * instance and name.
 * @type {Record<string, Record<string, [Type[], Type[], (params: Value[]) => InputValue[]]>>}
 */
const handlers = {
  "": {
    ping: [[], [t.string], () => ["pong"]],
  },
  "wrpc-test:integration/foo": {
    f: [[t.string], [t.u32], ([x]) => (x === "f" ? [42] : fail(`expected \`f\`, got \`${x}\``))],
    foo: [[t.string], [], ([x]) => (x === "foo" ? [] : fail(`expected \`foo\`, got \`${x}\``))],
  },
  "wrpc-test:integration/sync": {
    fallible: [
      [t.bool],
      [t.result(t.bool, t.string)],
      ([ok]) => [ok ? { kind: "ok", value: true } : { kind: "error", value: "test" }],
    ],
    numbers: [
      [],
      [t.tuple(t.u8, t.u16, t.u32, t.u64, t.s8, t.s16, t.s32, t.s64, t.f32, t.f64)],
      () => [[1, 2, 3, 4n, 5, 6, 7, 8n, 9, 10]],
    ],
    "with-flags": [
      [t.bool, t.bool, t.bool],
      [t.flags(["a", "b", "c"])],
      ([a, b, c]) => [{ a, b, c }],
    ],
    "with-variant-option": [
      [t.bool],
      [t.option(v)],
      ([ok]) => [ok ? { kind: "var", value: r("bar") } : undefined],
    ],
    "with-variant-list": [
      [],
      [t.list(v)],
      () => [
        [
          { kind: "empty" },
          { kind: "var", value: r("foo") },
          { kind: "empty" },
          { kind: "empty" },
          { kind: "empty" },
          { kind: "var", value: r("bar") },
        ],
      ],
    ],
    "with-record": [[], [rec], () => [r("foo")]],
    "with-record-list": [
      [t.u8],
      [t.list(rec)],
      ([n]) => [Array.from({ length: /** @type {number} */ (n) }, (_, i) => r(`${i}`))],
    ],
    "with-record-tuple": [[], [t.tuple(rec, rec)], () => [[r("0"), r("1")]]],
    "with-enum": [[], [t.enum(["foo", "bar"])], () => ["bar"]],
    "identity-char": [[t.char], [t.char], ([c]) => [c]],
    "identity-option-lists": [
      [t.option(t.list(t.u8)), t.option(t.list(t.string))],
      [t.tuple(t.option(t.list(t.u8)), t.option(t.list(t.string)))],
      (v) => [v],
    ],
  },
  "wrpc-test:integration/resources": {
    foo: [[], [t.own("foo")], () => [fooHandle]],
    "foo.foo": [[t.own("foo")], [t.string], ([v]) => withFoo(v, ["foo"])],
    "foo.bar": [[t.borrow("foo")], [t.string], ([v]) => withFoo(v, ["bar"])],
    bar: [[t.borrow("foo")], [t.string], ([v]) => withFoo(v, ["bar"])],
    counter: [[t.u32], [t.own("counter")], ([n]) => [newCounter(/** @type {number} */ (n))]],
    "counter.clone-counter": [
      [t.borrow("counter")],
      [t.own("counter")],
      ([v]) => [newCounter(counters[counter(v)])],
    ],
    "counter.get-count": [[t.borrow("counter")], [t.u32], ([v]) => [counters[counter(v)]]],
    "counter.increment-by": [
      [t.borrow("counter"), t.u32],
      [],
      ([v, n]) => {
        const i = counter(v);
        counters[i] = (counters[i] + /** @type {number} */ (n)) >>> 0;
        return [];
      },
    ],
    "counter.sum": [
      [t.borrow("counter"), t.borrow("counter")],
      [t.u32],
      ([a, b]) => [(counters[counter(a)] + counters[counter(b)]) >>> 0],
    ],
  },
  "wrpc-test:integration/strange": {
    bar: [[t.borrow("foo")], [t.u64], ([v]) => withFoo(v, [42n])],
  },
  "wrpc-test:integration/async": {
    "with-streams": [
      [],
      [t.tuple(t.stream(t.u8), t.stream(t.list(t.string)))],
      () => [[new TextEncoder().encode("test"), [[["foo"]], [["bar", "baz"]]]]],
    ],
    "with-future": [
      [t.record({ foo: t.string }), t.stream(t.u8)],
      [t.future(t.stream(t.u8))],
      ([x, s]) => {
        const { foo } = /** @type {{ foo: string }} */ (x);
        return foo === "bar" ? [s] : fail(`expected \`bar\`, got \`${foo}\``);
      },
    ],
    "identity-future": [[t.future()], [t.future()], ([v]) => [v]],
    "identity-cases": [cases, [t.tuple(...cases)], (params) => [params]],
  },
};

/** @param {string} msg @returns {never} */
function fail(msg) {
  throw new Error(msg);
}

listen(process.argv[2], async (transport) => {
  let inv;
  try {
    inv = await accept(transport);
    const handler =
      handlers[inv.instance]?.[inv.func] ??
      fail(`unknown function \`${inv.instance}.${inv.func}\``);
    const [paramTypes, resultTypes, f] = handler;
    const { params, done } = await inv.receiveParams(paramTypes);
    await Promise.all([done, (async () => inv.sendResults(resultTypes, f(params)))()]);
    await inv.finished;
  } catch (err) {
    inv?.mux.abort(err);
    console.error(`failed to serve \`${inv?.instance}.${inv?.func}\`:`, err);
  }
});
