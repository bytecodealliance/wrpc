// The HTTP transport: an invocation per `POST` request, whose request body
// carries the client's frames and whose response body carries the server's.

import { next, receive } from "./codec.js";
import { HeaderDecodeError, InvokeError, ReceiveError, ServeError } from "./error.js";
import { FrameReader, FrameWriter } from "./frame.js";
import { handle, mapError, never, noop, requestError, responseError, send } from "./rpc.js";

/** The media type of wRPC request and response bodies. */
export const MEDIA_TYPE = "application/vnd.bytecodealliance.wrpc";

/**
 * @typedef {import("./rpc.js").Codec} Codec
 * @typedef {Omit<RequestInit, "method" | "body"> & { fetch?: typeof fetch, buffer?: boolean }} FetchOptions
 */

/**
 * The promise resolving once `rx` ends after `received` resolved, failing on
 * any data following the invocation.
 * @param {FrameReader} rx @param {Promise<void>} received
 */
const end = (rx, received) =>
  received.then(async () => {
    if ((await next(rx)) !== null) throw new ReceiveError("dataAfterEnd");
  });

/**
 * A client invoking each function with a `POST` request to `input` with
 * `fetch`, with the {@link MEDIA_TYPE} unless `headers` set one, reading the
 * body of the response, whatever its status. To act on the response, wrap
 * `fetch`, failing the invocation by rejecting, with an {@link InvokeError} of
 * kind `request` caused by the rejection. The other `options` are passed to
 * `fetch`. The request body is streamed, unless `buffer` is set: then it is
 * sent once the parameters are transmitted, for runtimes which cannot stream
 * request bodies.
 * @param {Parameters<typeof fetch>[0]} input
 * @param {FetchOptions} [options]
 */
export function fromFetch(input, { fetch = globalThis.fetch, buffer = false, ...init } = {}) {
  return {
    /**
     * @param {Codec} codec @param {string} instance @param {string} name @param {any} params
     * @returns {Promise<[any, Promise<void>]>}
     */
    async invoke({ enc, dec }, instance, name, params) {
      /** @type {Uint8Array[]} */
      const chunks = [];
      const pipe = buffer ? undefined : new TransformStream();
      const tx = new FrameWriter(
        pipe?.writable ?? new WritableStream({ write: (chunk) => void chunks.push(chunk) }),
      );
      const request = (/** @type {unknown} */ err) => new InvokeError("request", err);
      const start = send(enc, tx, params, request);
      tx.header({ instance, name }).catch(noop);
      const sent = start().then(() => mapError(tx.writer.close(), request));
      const headers = new Headers(init.headers ?? /** @type {Request} */ (input).headers);
      if (!headers.has("content-type")) headers.set("content-type", MEDIA_TYPE);
      const req = { ...init, headers, method: "POST" };
      const res = pipe
        ? (async () =>
            fetch(
              input,
              /** @type {RequestInit} */ ({ ...req, body: pipe.readable, duplex: "half" }),
            ))()
        : sent.then(() =>
            fetch(input, {
              ...req,
              body: new Blob(/** @type {Uint8Array<ArrayBuffer>[]} */ (chunks)),
            }),
          );
      const rx = res.then(
        (res) => new FrameReader(res.body ?? new Blob([]).stream()),
        (err) => {
          throw request(err);
        },
      );
      /** @param {unknown} err */
      const fail = (err) => {
        tx.writer.abort(err).catch(noop);
        rx.then((rx) => rx.reader.cancel(err)).catch(noop);
        throw err;
      };
      try {
        const [results, received] = await Promise.race([
          rx.then(async (rx) => {
            const [results, received] = await mapError(receive(dec, rx), responseError);
            return /** @type {const} */ ([results, mapError(end(rx, received), responseError)]);
          }),
          sent.then(() => never),
        ]);
        return [results, Promise.all([sent, received]).then(noop, fail)];
      } catch (err) {
        return fail(err);
      }
    },
  };
}

/**
 * Receives the invocation header of `request`, returning it along with the
 * request to serve the invocation over.
 * @param {Request} request
 * @param {{ maxDepth?: number, maxInstanceLength?: number, maxNameLength?: number }} [options]
 * @returns {Promise<[{ instance: string, name: string }, IncomingRequest]>}
 */
export async function accept(request, options) {
  const rx = new FrameReader(request.body ?? new Blob([]).stream(), options);
  try {
    const header = await rx.header(options);
    if (!header) throw new HeaderDecodeError("truncated");
    return [header, new IncomingRequest(rx)];
  } catch (err) {
    rx.reader.cancel(err).catch(noop);
    throw err;
  }
}

/** A request carrying an invocation, served by responding with its results. */
export class IncomingRequest {
  #rx;

  /** @param {FrameReader} rx */
  constructor(rx) {
    this.#rx = rx;
  }

  /**
   * @param {Codec} codec @param {(params: any) => any} f
   * @returns {Promise<Response>}
   */
  async serve({ enc, dec }, f) {
    const rx = this.#rx;
    const [params, rest] = await mapError(receive(dec, rx), (err) => {
      rx.reader.cancel(err).catch(noop);
      return requestError(err);
    });
    const ended = mapError(end(rx, rest), requestError);
    const { readable, writable } = new TransformStream();
    const tx = new FrameWriter(writable);
    /** @param {unknown} err */
    const fail = (err) => {
      tx.writer.abort(err).catch(noop);
      rx.reader.cancel(err).catch(noop);
      throw err;
    };
    try {
      const results = await Promise.race([handle(f, params), ended.then(() => never)]);
      const sent = send(enc, tx, results, (err) => new ServeError("response", err))();
      Promise.all([ended, sent])
        .then(() => tx.writer.close(), fail)
        .catch(noop);
    } catch (err) {
      return fail(err);
    }
    return new Response(readable, { headers: { "content-type": MEDIA_TYPE } });
  }
}
