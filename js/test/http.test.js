import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { fromFetch, fromRequest } from "../src/http.js";
import { accept, invoke } from "../src/session.js";

const ADDR = "http://localhost/rpc";

test("values echo over HTTP", async () => {
  const ty = t.tuple(t.string, t.stream(t.u8));
  for (const buffer of [false, true]) {
    /** @type {Promise<unknown>[]} */
    const serving = [];
    /** @type {typeof globalThis.fetch} */
    const fetch = async (input, init) => {
      const { transport, response } = fromRequest(new Request(input, init));
      serving.push(
        (async () => {
          const inv = await accept(transport);
          const { params, done } = await inv.receiveParams([ty]);
          await Promise.all([done, inv.sendResults([ty], params), inv.finished]);
        })(),
      );
      return response;
    };
    const { results, done, finished } = await invoke(
      fromFetch(ADDR, { fetch, buffer }),
      "i",
      "f",
      [ty],
      [["hi", Uint8Array.of(1, 2)]],
      [ty],
    );
    const [s, bytes] = /** @type {[string, AsyncIterable<Uint8Array>]} */ (results[0]);
    const chunks = [];
    for await (const chunk of bytes) chunks.push(...chunk);
    assert.deepEqual([s, chunks], ["hi", [1, 2]]);
    await Promise.all([done, finished, ...serving]);
  }
});

test("a response status other than 2xx fails the invocation", async () => {
  /** @type {typeof globalThis.fetch} */
  const fetch = async () => new Response("nope", { status: 404 });
  await assert.rejects(invoke(fromFetch(ADDR, { fetch }), "i", "f", [], [], [t.u8]), {
    message: "HTTP request failed with status 404",
  });
});
