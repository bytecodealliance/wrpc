// Invoking and serving functions over the frames of an invocation, using the
// codec tree.

import { UnreadError, decoder, encoder, receive, transmission } from "./codec.js";
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

const MESSAGES = {
  request: "failed to receive request",
  unread: "handler dropped an unread parameter",
  handler: "handler failed",
  response: "failed to send response",
};

/** The error of serving an invocation, by the side at fault. */
export class ServeError extends Error {
  /** @param {keyof typeof MESSAGES} kind @param {unknown} [cause] */
  constructor(kind, cause) {
    super(MESSAGES[kind], cause === undefined ? undefined : { cause });
    this.name = "ServeError";
    this.kind = kind;
  }
}

/** @param {unknown} err */
export const requestError = (err) =>
  err instanceof UnreadError ? new ServeError("unread") : new ServeError("request", err);

/**
 * Invokes `instance#name` with `params` using `codec` over the frames written
 * by `tx` and read by `rx`.
 * @param {Codec} codec @param {FrameWriter} tx @param {FrameReader} rx
 * @param {string} instance @param {string} name @param {any} params
 * @returns {Promise<[any, Promise<void>]>}
 */
export async function invokeOver({ enc, dec }, tx, rx, instance, name, params) {
  const send = transmission(enc, tx, params);
  tx.header({ instance, name }).catch(noop);
  const sent = send();
  const [results, received] = await Promise.race([receive(dec, rx), sent.then(() => never)]);
  return [results, Promise.all([sent, received]).then(noop)];
}

/**
 * Calls `f` with the parameters received by `rx` using `codec` and transmits
 * the results it returns using `tx`.
 * @param {Codec} codec @param {FrameWriter} tx @param {FrameReader} rx
 * @param {(params: any) => any} f
 * @returns {Promise<void>}
 */
export async function serveOver({ enc, dec }, tx, rx, f) {
  /** @type {[any, Promise<void>]} */
  let received;
  try {
    received = await receive(dec, rx);
  } catch (err) {
    throw requestError(err);
  }
  const [params, rest] = received;
  await Promise.all([
    rest.catch((err) => {
      throw requestError(err);
    }),
    (async () => {
      const results = await handle(f, params);
      /** @type {() => Promise<void>} */
      let send;
      try {
        send = transmission(enc, tx, results);
      } catch (err) {
        throw new ServeError("response", err);
      }
      await send().catch((err) => {
        throw new ServeError("response", err);
      });
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
