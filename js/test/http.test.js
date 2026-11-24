import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { MEDIA_TYPE, accept, fromFetch } from "../src/http.js";
import { invoke, serve } from "../src/rpc.js";

const ADDR = "http://localhost/rpc";

/**
 * A `fetch` serving each request with `f`, collecting the responses' bodies' ends.
 * @param {(req: Request) => Promise<Response>} f
 */
function server(f) {
  /** @type {Promise<unknown>[]} */
  const served = [];
  /** @type {typeof globalThis.fetch} */
  const fetch = async (input, init) => {
    const res = await f(new Request(input, init));
    const [body, rest] = /** @type {ReadableStream} */ (res.body).tee();
    served.push(new Response(rest).arrayBuffer());
    return new Response(body, res);
  };
  return { fetch, served };
}

test("values echo over HTTP", async () => {
  const ty = t.tuple(t.string, t.stream(t.u8));
  for (const buffer of [false, true]) {
    const { fetch, served } = server(async (req) => {
      const [header, srv] = await accept(req);
      assert.deepEqual(header, { instance: "i", name: "f" });
      return serve(srv, [ty], [ty], (v) => [v]);
    });
    const [[[s, bytes]], io] = await invoke(
      fromFetch(ADDR, { fetch, buffer }),
      "i",
      "f",
      [ty],
      [["hi", Uint8Array.of(1, 2)]],
      [ty],
    );
    const chunks = [];
    for await (const chunk of /** @type {ReadableStream<Uint8Array>} */ (bytes)) {
      chunks.push(...chunk);
    }
    assert.deepEqual([s, chunks], ["hi", [1, 2]]);
    await Promise.all([io, ...served]);
  }
});

test("callers decide on the response", async () => {
  /** @type {Response | undefined} */
  let res;
  /** @type {typeof globalThis.fetch} */
  const fetch = async (input, init) => {
    const req = new Request(input, init);
    assert.equal(req.headers.get("content-type"), MEDIA_TYPE);
    const [, srv] = await accept(req);
    const response = await serve(srv, [], [t.u8], () => [42]);
    response.headers.set("foo", "bar");
    return (res = new Response(response.body, { status: 418, headers: response.headers }));
  };
  const [results, io] = await invoke(fromFetch(ADDR, { fetch }), "i", "f", [], [], [t.u8]);
  assert.deepEqual(results, [42]);
  await io;
  assert.equal(res?.status, 418);
  assert.equal(res?.headers.get("content-type"), MEDIA_TYPE);
  assert.equal(res?.headers.get("foo"), "bar");

  /** @type {typeof globalThis.fetch} */
  const reject = async () => {
    const res = new Response("nope", { status: 418 });
    if (!res.ok) throw new Error(`status ${res.status}`);
    return res;
  };
  const err = await invoke(fromFetch(ADDR, { fetch: reject }), "i", "f", [], [], [t.u8]).catch(
    (err) => err,
  );
  assert.equal(err.kind, "request");
  assert.equal(err.cause.message, "status 418");
});

test("data after the invocation fails it", async () => {
  /** @param {Request} req */
  const respond = async (req) => {
    const [, srv] = await accept(req);
    return serve(srv, [], [], () => []);
  };
  /** @param {typeof globalThis.fetch} fetch */
  const call = async (fetch) => {
    const [, io] = await invoke(fromFetch(ADDR, { fetch, buffer: true }), "i", "f", [], [], []);
    await io;
  };

  /** @type {Promise<ArrayBuffer> | undefined} */
  let served;
  /** @type {typeof globalThis.fetch} */
  const twice = async (input, init) => {
    const body = await new Request(input, init).arrayBuffer();
    const res = await respond(new Request(input, { ...init, body: new Blob([body, body]) }));
    served = res.arrayBuffer();
    return new Response();
  };
  await call(twice).catch(() => {});
  await assert.rejects(/** @type {Promise<unknown>} */ (served), (/** @type {any} */ err) => {
    assert.equal(err.kind, "request");
    assert.equal(err.cause.kind, "dataAfterEnd");
    return true;
  });

  /** @type {typeof globalThis.fetch} */
  const trailing = async (input, init) => {
    const body = await (await respond(new Request(input, init))).arrayBuffer();
    return new Response(new Blob([body, Uint8Array.of(0, 0)]));
  };
  await assert.rejects(call(trailing), (/** @type {any} */ err) => {
    assert.equal(err.kind, "response");
    assert.equal(err.cause.kind, "dataAfterEnd");
    return true;
  });
});
