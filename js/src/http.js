// The HTTP transport: one `POST` per invocation, whose request body carries
// the client's data and whose response body carries the server's.

import { fromWebStreams } from "./mux.js";

/**
 * @typedef {import("./mux.js").Transport} Transport
 * @typedef {Omit<RequestInit, "method" | "body" | "signal"> & { fetch?: typeof fetch, buffer?: boolean }} FetchOptions
 */

/**
 * A client {@link Transport} sending a `POST` request to `input` with `fetch`,
 * whose response must have a `2xx` status, or the invocation fails with an
 * error carrying the status and the first 4 KiB of the response body. The
 * other `options` are passed to `fetch`. The request body is streamed, unless
 * `buffer` is set: then it is sent once the client has ended transmit, for
 * runtimes which cannot stream request bodies.
 * @param {Parameters<typeof fetch>[0]} input
 * @param {FetchOptions} [options]
 * @returns {Transport}
 */
export function fromFetch(input, { fetch = globalThis.fetch, buffer = false, ...init } = {}) {
  const ctrl = new AbortController();
  /** @type {TransformStreamDefaultController<Uint8Array> | undefined} */
  let controller;
  const stream = buffer ? undefined : new TransformStream({ start: (c) => void (controller = c) });
  /** @type {(body: RequestInit["body"]) => void} */
  let send = () => {};
  /** @type {(reason: unknown) => void} */
  let fail = () => {};
  /** @type {Promise<ReadableStreamDefaultReader<Uint8Array>>} */
  const reader = new Promise((resolve, reject) => {
    send = resolve;
    fail = reject;
  })
    .then((body) => {
      const req = { ...init, method: "POST", body, signal: ctrl.signal };
      return fetch(input, buffer ? req : /** @type {RequestInit} */ ({ ...req, duplex: "half" }));
    })
    .then(async (res) => {
      if (!res.ok) {
        const text = res.body ? await readPrefix(res.body, 4096) : "";
        throw new Error(`HTTP request failed with status ${res.status}${text ? `: ${text}` : ""}`);
      }
      return (res.body ?? new Blob([]).stream()).getReader();
    });
  /** @param {unknown} reason */
  const abort = (reason) => {
    fail(reason);
    ctrl.abort(reason);
    controller?.error(reason);
    reader.then((r) => r.cancel(reason)).catch(() => {});
  };
  reader.catch(abort);
  /** @type {Pick<Transport, "write" | "closeWrite">} */
  let body;
  if (stream === undefined) {
    /** @type {Uint8Array[]} */
    const chunks = [];
    body = {
      write: (bytes) => void chunks.push(bytes),
      closeWrite: () => send(new Blob(/** @type {Uint8Array<ArrayBuffer>[]} */ (chunks))),
    };
  } else {
    const writer = stream.writable.getWriter();
    send(stream.readable);
    body = { write: (bytes) => writer.write(bytes), closeWrite: () => writer.close() };
  }
  return {
    ...body,
    read: async () => {
      const { value, done } = await (await reader).read();
      return done ? null : value;
    },
    abort,
  };
}

/**
 * Reads at most `limit` bytes of `body` as UTF-8 text and cancels it.
 * @param {ReadableStream<Uint8Array>} body
 * @param {number} limit
 * @returns {Promise<string>}
 */
async function readPrefix(body, limit) {
  const reader = body.getReader();
  const buf = new Uint8Array(limit);
  let n = 0;
  while (n < limit) {
    const r = await reader.read().catch(() => null);
    if (!r || r.done) break;
    const k = Math.min(r.value.length, limit - n);
    buf.set(r.value.subarray(0, k), n);
    n += k;
  }
  reader.cancel().catch(() => {});
  return new TextDecoder().decode(buf.subarray(0, n));
}

/**
 * A server {@link Transport} over the body of `request`, returned with the
 * `response` carrying the server's data, which must be sent without waiting
 * for the request body to complete.
 * @param {Request} request
 * @returns {{ transport: Transport, response: Response }}
 */
export function fromRequest(request) {
  const { readable, writable } = new TransformStream();
  return {
    transport: fromWebStreams({ readable: request.body ?? new Blob([]).stream(), writable }),
    response: new Response(readable),
  };
}
