// Serves `echo-async` of the `echo-server` world of `tests/js/wit` and the
// `async-server` world of `tests/wit` over the codec tree on the address given
// as the first argument, for the Rust client in `tests/js.rs`, with the
// bindings of `echo-server` in the directory given as the second argument.

import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { t } from "../../js/src/index.js";
import { listenFramed, serveFramed } from "./transport.js";

/**
 * @typedef {import("../../js/src/index.js").Type} Type
 * @typedef {import("../../js/src/index.js").InputValue} InputValue
 */

const [addr, bindings] = process.argv.slice(2);

const ASYNC = "wrpc-test:integration/async";
const something = t.record({ foo: t.string });
const asyncCase = t.variant({
  empty: null,
  fut: t.future(t.string),
  elements: t.list(t.future(t.string)),
  pair: t.tuple(t.u8, t.future(t.string)),
  chunks: t.stream(t.future(t.string)),
});
const nested = t.future(t.future(t.future(t.stream(t.string))));
const cases = [t.option(t.future()), t.result(t.stream(t.u8), t.future(t.u32)), asyncCase];

/** @type {Record<string, [Type[], Type[], (...params: any[]) => InputValue[]]>} */
const functions = {
  [`${ASYNC}#with-streams`]: [
    [],
    [t.tuple(t.stream(t.u8), t.stream(t.list(t.string)))],
    () => [[new TextEncoder().encode("test"), [[["foo"]], [["bar", "baz"]]]]],
  ],
  [`${ASYNC}#with-future`]: [
    [something, t.stream(t.u8)],
    [t.future(t.stream(t.u8))],
    (x, s) => (x.foo === "bar" ? [s] : fail(`expected \`bar\`, got \`${x.foo}\``)),
  ],
  [`${ASYNC}#identity-nested-async`]: [[nested], [nested], (v) => [v]],
  [`${ASYNC}#identity-future`]: [[t.future()], [t.future()], (v) => [v]],
  [`${ASYNC}#identity-cases`]: [cases, [t.tuple(...cases)], (...v) => [v]],
};

/** @type {Promise<typeof functions>} */
const served = import(
  pathToFileURL(join(bindings, "echo-server/exports/wrpc-test/js/echo.js")).href
).then(({ Asyncs }) => ({
  ...functions,
  "wrpc-test:js/echo#echo-async": [[Asyncs], [Asyncs], (v) => [v]],
}));

/** @param {string} msg @returns {never} */
function fail(msg) {
  throw new Error(msg);
}

listenFramed(addr, async (lis, sock) => {
  try {
    const functions = await served;
    for (let inv; (inv = await lis.accept());) {
      const [{ instance, name }, io] = inv;
      const f = functions[`${instance}#${name}`] ?? fail(`\`${instance}#${name}\` is not served`);
      await serveFramed(io, ...f);
    }
    await lis.io.tx.writer.close();
  } catch (err) {
    console.error("failed to serve:", err);
    sock.resetAndDestroy();
  }
});
