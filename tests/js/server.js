// Serves the `sync-server` and `async-server` worlds of `tests/wit` on the
// address given as the first argument, for the Rust clients in `tests/js.rs`.

import { accept, t } from "../../js/src/index.js";
import { listen } from "./tcp.js";

const rec = t.record({ nested: t.record({ foo: t.string }) });
const v = t.variant({ var: rec, empty: null });
/** @param {string} foo */
const r = (foo) => ({ nested: { foo } });

/**
 * The parameter and result types and the implementation of each function, by
 * instance and name.
 * @type {Record<string, Record<string, [any[], any[], (params: any[]) => any[]]>>}
 */
const handlers = {
  "wrpc-test:integration/foo": {
    f: [[t.string], [t.u32], ([x]) => (x === "f" ? [42] : fail(`expected \`f\`, got \`${x}\``))],
    foo: [[t.string], [], ([x]) => (x === "foo" ? [] : fail(`expected \`foo\`, got \`${x}\``))],
  },
  "wrpc-test:integration/sync": {
    fallible: [
      [t.bool],
      [t.result(t.bool, t.string)],
      ([ok]) => [ok ? { tag: "ok", val: true } : { tag: "err", val: "test" }],
    ],
    numbers: [
      [],
      [t.tuple(t.u8, t.u16, t.u32, t.u64, t.s8, t.s16, t.s32, t.s64, t.f32, t.f64)],
      () => [[1, 2, 3, 4n, 5, 6, 7, 8n, 9, 10]],
    ],
    "with-flags": [[t.bool, t.bool, t.bool], [t.flags(["a", "b", "c"])], ([a, b, c]) => [{ a, b, c }]],
    "with-variant-option": [
      [t.bool],
      [t.option(v)],
      ([ok]) => [ok ? { tag: "var", val: r("bar") } : undefined],
    ],
    "with-variant-list": [
      [],
      [t.list(v)],
      () => [["empty", { tag: "var", val: r("foo") }, "empty", "empty", "empty", { tag: "var", val: r("bar") }]],
    ],
    "with-record": [[], [rec], () => [r("foo")]],
    "with-record-list": [[t.u8], [t.list(rec)], ([n]) => [Array.from({ length: n }, (_, i) => r(`${i}`))]],
    "with-record-tuple": [[], [t.tuple(rec, rec)], () => [[r("0"), r("1")]]],
    "with-enum": [[], [t.enum(["foo", "bar"])], () => ["bar"]],
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
      ([x, s]) => (x.foo === "bar" ? [s] : fail(`expected \`bar\`, got \`${x.foo}\``)),
    ],
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
    const handler = handlers[inv.instance]?.[inv.func] ?? fail(`unknown function \`${inv.instance}.${inv.func}\``);
    const [paramTypes, resultTypes, f] = handler;
    const { params, done } = await inv.receiveParams(paramTypes);
    await Promise.all([done, (async () => inv.sendResults(resultTypes, f(params)))()]);
  } catch (err) {
    inv?.mux.abort(err);
    console.error(`failed to serve \`${inv?.instance}.${inv?.func}\`:`, err);
  }
});
