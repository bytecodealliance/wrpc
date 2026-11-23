import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { MEDIA_TYPE, fromFetch, fromRequest } from "../src/http.js";
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

test("callers decide on the response", async () => {
  /** @type {Promise<unknown>[]} */
  const serving = [];
  /** @type {typeof globalThis.fetch} */
  const serve = async (input, init) => {
    const req = new Request(input, init);
    assert.equal(req.headers.get("content-type"), MEDIA_TYPE);
    const { transport, response } = fromRequest(req);
    serving.push(
      (async () => {
        const inv = await accept(transport);
        const { done } = await inv.receiveParams([]);
        await Promise.all([done, inv.sendResults([t.u8], [42]), inv.finished]);
      })(),
    );
    response.headers.set("foo", "bar");
    return new Response(response.body, { status: 418, headers: response.headers });
  };
  /** @type {Response | undefined} */
  let res;
  /** @type {typeof globalThis.fetch} */
  const fetch = async (input, init) => (res = await serve(input, init));
  const { results, finished } = await invoke(fromFetch(ADDR, { fetch }), "i", "f", [], [], [t.u8]);
  assert.deepEqual(results, [42]);
  await Promise.all([finished, ...serving]);
  assert.equal(res?.status, 418);
  assert.equal(res?.headers.get("content-type"), MEDIA_TYPE);
  assert.equal(res?.headers.get("foo"), "bar");

  /** @type {typeof globalThis.fetch} */
  const reject = async () => {
    const res = new Response("nope", { status: 418 });
    if (!res.ok) throw new Error(`status ${res.status}`);
    return res;
  };
  await assert.rejects(invoke(fromFetch(ADDR, { fetch: reject }), "i", "f", [], [], [t.u8]), {
    message: "status 418",
  });
});

test("data after the invocation fails it", async () => {
  const err = { message: "unexpected data after the invocation" };
  /** @type {Promise<void>} */
  let served = Promise.resolve();
  /** @param {Request} req */
  const serve = (req) => {
    const { transport, response } = fromRequest(req);
    served = (async () => {
      const inv = await accept(transport);
      const { done } = await inv.receiveParams([]);
      await Promise.all([done, inv.sendResults([], []), inv.finished]);
    })();
    served.catch(() => {});
    return response;
  };
  /** @param {typeof globalThis.fetch} fetch */
  const call = (fetch) =>
    invoke(fromFetch(ADDR, { fetch, buffer: true }), "i", "f", [], []).then((r) => r.finished);

  /** @type {typeof globalThis.fetch} */
  const twice = async (input, init) => {
    const body = await new Request(input, init).arrayBuffer();
    return serve(new Request(input, { ...init, body: new Blob([body, body]) }));
  };
  await call(twice).catch(() => {});
  await assert.rejects(served, err);

  /** @type {typeof globalThis.fetch} */
  const trailing = async (input, init) => {
    const body = await serve(new Request(input, init)).arrayBuffer();
    return new Response(new Blob([body, Uint8Array.of(0)]));
  };
  await assert.rejects(call(trailing), err);
  await served;
});
