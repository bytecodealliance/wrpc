// Invoking and serving functions over the frames of an invocation, using the
// codec tree.

import { decoder, encoder, receive, transmission } from "./codec.js";
import { InvokeError, ReceiveError, ServeError } from "./error.js";
import { tuple } from "./types.js";

/**
 * @typedef {import("./codec.js").Encoder} Encoder
 * @typedef {import("./codec.js").Decoder} Decoder
 * @typedef {{ enc: Encoder, dec: Decoder }} Codec
 * @typedef {import("./frame.js").FrameWriter} FrameWriter
 * @typedef {import("./frame.js").FrameReader} FrameReader
 * @typedef {import("./index.d.ts").Invoke} Invoke
 * @typedef {import("./index.d.ts").Type} Type
 */

export const noop = () => {};

/** @type {Promise<never>} */
export const never = new Promise(noop);

/**
 * `p`, failing with what `fail` maps its error to.
 * @template T @param {Promise<T>} p @param {(err: unknown) => Error} fail
 * @returns {Promise<T>}
 */
export const mapError = (p, fail) =>
  p.catch((err) => {
    throw fail(err);
  });

/** @param {unknown} err */
const isUnread = (err) => err instanceof ReceiveError && err.kind === "unread";

/** The error of receiving the parameters of an invocation with `err`. @param {unknown} err */
export const requestError = (err) =>
  isUnread(err) ? new ServeError("unread") : new ServeError("request", err);

/** The error of receiving the results of an invocation with `err`. @param {unknown} err */
export const responseError = (err) =>
  isUnread(err) ? new InvokeError("unread") : new InvokeError("response", err);

/**
 * Encodes `value` using `enc`, returning the function sending it using `tx`,
 * failing with what `fail` maps the error to.
 * @param {Encoder} enc @param {FrameWriter} tx @param {any} value
 * @param {(err: unknown) => Error} fail
 * @returns {() => Promise<void>}
 */
export function send(enc, tx, value, fail) {
  /** @type {() => Promise<void>} */
  let start;
  try {
    start = transmission(enc, tx, value);
  } catch (err) {
    throw fail(err);
  }
  return () => mapError(start(), fail);
}

/**
 * Invokes `instance#name` with `params` using `codec` over the frames written
 * by `tx` and read by `rx`.
 * @param {Codec} codec @param {FrameWriter} tx @param {FrameReader} rx
 * @param {string} instance @param {string} name @param {any} params
 * @returns {Promise<[any, Promise<void>]>}
 */
export async function invokeOver({ enc, dec }, tx, rx, instance, name, params) {
  const start = send(enc, tx, params, (err) => new InvokeError("request", err));
  tx.header({ instance, name }).catch(noop);
  const sent = start();
  const [results, received] = await Promise.race([
    mapError(receive(dec, rx), responseError),
    sent.then(() => never),
  ]);
  return [results, Promise.all([sent, mapError(received, responseError)]).then(noop)];
}

/**
 * Calls `f` with the parameters received by `rx` using `codec` and transmits
 * the results it returns using `tx`.
 * @param {Codec} codec @param {FrameWriter} tx @param {FrameReader} rx
 * @param {(params: any) => any} f
 * @returns {Promise<void>}
 */
export async function serveOver({ enc, dec }, tx, rx, f) {
  const [params, rest] = await mapError(receive(dec, rx), requestError);
  await Promise.all([
    mapError(rest, requestError),
    (async () => {
      const results = await handle(f, params);
      await send(enc, tx, results, (err) => new ServeError("response", err))();
    })(),
  ]);
}

/** @param {(params: any) => any} f @param {any} params */
export async function handle(f, params) {
  try {
    return await f(params);
  } catch (err) {
    throw new ServeError("handler", err);
  }
}

/**
 * The encoder of the values of `types`, failing on any other number of values.
 * @param {Type[]} types @returns {Encoder}
 */
function valuesEncoder(types) {
  const enc = encoder(tuple(...types));
  return {
    encode(w, vs) {
      if (vs.length !== types.length) {
        throw new RangeError(`expected ${types.length} values, got ${vs.length}`);
      }
      enc.encode(w, vs);
    },
    transmit: (path, tx) => enc.transmit(path, tx),
    get complete() {
      return enc.complete;
    },
  };
}

/**
 * Invokes `instance#name` with `values` of `params` using `clt`, returning the
 * `results` and the promise of the rest of the invocation.
 * @param {Invoke} clt @param {string} instance @param {string} name
 * @param {Type[]} params @param {any[]} values @param {Type[]} results
 * @returns {Promise<[any[], Promise<void>]>}
 */
export async function invoke(clt, instance, name, params, values, results) {
  const codec = { enc: valuesEncoder(params), dec: decoder(tuple(...results)) };
  return clt.invoke(codec, instance, name, values);
}

/**
 * Serves an invocation using `srv` by calling `f` with its `params` and
 * transmitting the `results` it returns.
 * @template O
 * @param {import("./index.d.ts").Serve<O>} srv @param {Type[]} params @param {Type[]} results
 * @param {(...params: any[]) => any} f
 * @returns {Promise<O>}
 */
export async function serve(srv, params, results, f) {
  const codec = { enc: valuesEncoder(results), dec: decoder(tuple(...params)) };
  return srv.serve(codec, (/** @type {any[]} */ params) => f(...params));
}
