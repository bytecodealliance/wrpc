import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { accept, invoke } from "../src/session.js";
import { fromWebSocket } from "../src/websockets.js";

/** An in-memory `WebSocket`, connected to `peer`, delivering events in order. */
class Socket extends EventTarget {
  CONNECTING = 0;
  OPEN = 1;
  CLOSING = 2;
  CLOSED = 3;
  readyState = this.OPEN;
  binaryType = "blob";
  /** @type {Socket} */
  peer = this;

  /** @param {string | Uint8Array} data */
  send(data) {
    const msg = typeof data === "string" ? data : data.slice().buffer;
    queueMicrotask(() => this.peer.dispatchEvent(new MessageEvent("message", { data: msg })));
  }

  close(code = 1005, reason = "") {
    if (this.readyState !== this.OPEN) return;
    this.readyState = this.CLOSING;
    for (const s of [this, this.peer]) {
      queueMicrotask(() => {
        s.readyState = this.CLOSED;
        s.dispatchEvent(new CloseEvent("close", { code, reason }));
      });
    }
  }
}

/** @returns {[WebSocket, WebSocket]} */
function pair() {
  const [a, b] = [new Socket(), new Socket()];
  a.peer = b;
  b.peer = a;
  return /** @type {any} */ ([a, b]);
}

test("values echo over a WebSocket", { timeout: 1000 }, async () => {
  const ty = t.tuple(t.string, t.stream(t.u8));
  const [clt, srv] = pair();
  const serving = (async () => {
    const inv = await accept(fromWebSocket(srv));
    const { params, done } = await inv.receiveParams([ty]);
    await Promise.all([done, inv.sendResults([ty], params), inv.finished]);
  })();
  const { results, finished } = await invoke(
    fromWebSocket(clt),
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
  await Promise.all([finished, serving]);
  clt.close(1000);
});

test("writes are sent as messages of at most 1 MiB", async () => {
  const [clt, srv] = pair();
  /** @type {number[]} */
  const sizes = [];
  srv.addEventListener("message", (ev) =>
    sizes.push(/** @type {MessageEvent} */ (ev).data.byteLength),
  );
  await fromWebSocket(clt).write(new Uint8Array((1 << 20) + 1));
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sizes, [1 << 20, 1]);
});

test("a text message or a failure close fails the invocation", async () => {
  for (const [fail, message, code] of /** @type {const} */ ([
    [(/** @type {WebSocket} */ ws) => ws.send("x"), "unexpected WebSocket text message", 4000],
    [
      (/** @type {WebSocket} */ ws) => ws.close(4001, "no"),
      "WebSocket connection closed with code 4001: no",
      4001,
    ],
  ])) {
    const [clt, srv] = pair();
    /** @type {Promise<number>} */
    const closed = new Promise((resolve) =>
      clt.addEventListener("close", (ev) => resolve(/** @type {CloseEvent} */ (ev).code)),
    );
    fail(clt);
    await assert.rejects(accept(fromWebSocket(srv)), { message });
    assert.equal(await closed, code);
  }
});

test("more than 16 MiB received and not read fail the connection", async () => {
  const [clt, srv] = pair();
  const transport = fromWebSocket(srv);
  /** @type {Promise<number>} */
  const closed = new Promise((resolve) =>
    clt.addEventListener("close", (ev) => resolve(/** @type {CloseEvent} */ (ev).code)),
  );
  await fromWebSocket(clt).write(new Uint8Array((16 << 20) + 1));
  assert.equal(await closed, 4000);
  await assert.rejects(transport.read(), {
    message: "WebSocket receive buffer of 16 MiB exceeded",
  });
});

test("an already closed WebSocket fails the invocation", { timeout: 1000 }, async () => {
  const [clt, srv] = pair();
  clt.close(1000);
  await new Promise((resolve) => setImmediate(resolve));
  await assert.rejects(accept(fromWebSocket(srv)), { message: "WebSocket connection is closed" });
});
