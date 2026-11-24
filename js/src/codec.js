// The codec tree: the encoders and decoders of the values of a `Type`, which
// transmit and receive the pending values nested in the last value they
// encoded or decoded over the frames of an invocation (see `SPEC.md`,
// "Framed stream specification" and "Indexing").

import { Reader, UNEXPECTED_EOF, Writer, atomic, readVarU32 } from "./io.js";
import {
  decodeSteps,
  encodeValue,
  hasAsync,
  isTypedArray,
  iterableToValues,
  readChunk,
  toList,
  typedArray,
} from "./value.js";

/**
 * @typedef {import("./index.d.ts").Type} Type
 * @typedef {import("./index.d.ts").TypedArray} TypedArray
 * @typedef {import("./frame.js").FrameWriter} FrameWriter
 * @typedef {import("./frame.js").FrameReader} FrameReader
 * @typedef {{
 *   encode(w: Writer, v: any): void,
 *   transmit(path: readonly number[], tx: FrameWriter): Promise<void>,
 *   readonly complete: boolean,
 * }} Encoder
 * @typedef {{
 *   decode(r: Reader): Generator<number, any, void>,
 *   receive(path: readonly number[], at: number, data: Uint8Array): Promise<void> | void,
 *   readonly complete: boolean,
 * }} Decoder
 * @typedef {{ encoder(): Encoder, decoder(): Decoder, sync?: Type }} Codec
 */

const EMPTY = new Uint8Array(0);

/** Thrown on data for a pending value whose consumer was cancelled. */
export class UnreadError extends Error {
  constructor() {
    super("received data for a cancelled consumer");
    this.name = "UnreadError";
  }
}

/** The error for a frame received on `path`, which is not pending. @param {readonly number[]} path */
export const errorUnknownPath = (path) =>
  new RangeError(`received frame on unknown path \`[${path}]\``);

/** The error for data received after the end of a value. */
export const errorDataAfterEnd = () => new RangeError("received data after value end");

/** @param {string} msg */
const errorEof = (msg) => new RangeError(`${UNEXPECTED_EOF}: ${msg}`);

/** @param {Reader} r */
const isPending = (r) => {
  const b = r.u8();
  if (b > 1) throw new RangeError(`invalid status byte \`${b}\``);
  return b === 0;
};

/** A resumable decoder fed pushed data, buffering only the bytes of an incomplete step. */
class Feed {
  /** @type {any} */
  value;
  #r = new Reader(EMPTY);
  #steps;
  #need = 0;
  /** @type {Uint8Array[]} */
  #parts = [];
  #size = 0;

  /** @param {(r: Reader) => Generator<number, any, void>} decode */
  constructor(decode) {
    this.#steps = decode(this.#r);
  }

  /** @param {Uint8Array} data @returns {boolean} whether the value is decoded */
  push(data) {
    this.#parts.push(data);
    this.#size += data.length;
    if (this.#size < this.#need) return false;
    const r = this.#r;
    r.bytes = this.#parts.length === 1 ? this.#parts[0] : join(this.#parts, this.#size);
    r.pos = 0;
    const step = this.#steps.next();
    if (step.done) {
      this.value = step.value;
      return true;
    }
    this.#need = step.value;
    const rest = this.rest();
    this.#parts = [rest];
    this.#size = rest.length;
    return false;
  }

  /** The bytes pushed past the decoded value. */
  rest() {
    return this.#r.bytes.subarray(this.#r.pos);
  }
}

/** @param {Uint8Array[]} parts @param {number} size */
function join(parts, size) {
  const out = new Uint8Array(size);
  let off = 0;
  for (const part of parts) {
    out.set(part, off);
    off += part.length;
  }
  return out;
}

/**
 * @param {Map<number, Decoder>} nested @param {readonly number[]} path @param {number} at
 * @param {Uint8Array} data
 */
async function route(nested, path, at, data) {
  const dec = at < path.length ? nested.get(path[at]) : undefined;
  if (!dec) throw errorUnknownPath(path);
  await dec.receive(path, at + 1, data);
  if (dec.complete) nested.delete(path[at]);
}

/** @param {Map<number, Encoder>} nested @param {readonly number[]} path @param {FrameWriter} tx */
async function transmitNested(nested, path, tx) {
  await Promise.all(
    [...nested].map(async ([i, enc]) => {
      await enc.transmit([...path, i], tx);
      nested.delete(i);
    }),
  );
}

/** @param {Type} ty @returns {Codec} */
function sync(ty) {
  /** @type {Encoder} */
  const enc = {
    encode: (w, v) => encodeValue(w, ty, v),
    transmit: async () => {},
    complete: true,
  };
  /** @type {Decoder} */
  const dec = {
    decode: (r) => decodeSteps(r, ty),
    receive: (path) => {
      throw errorUnknownPath(path);
    },
    complete: true,
  };
  return { encoder: () => enc, decoder: () => dec, sync: ty };
}

/**
 * @param {Codec[]} codecs @param {(v: any) => any[]} split @param {(vs: any[]) => any} join
 * @returns {Codec}
 */
function product(codecs, split, join) {
  return {
    encoder() {
      /** @type {Map<number, Encoder>} */
      let nested = new Map();
      return {
        encode(w, v) {
          nested = new Map();
          const vs = split(v);
          codecs.forEach((c, i) => {
            const enc = c.encoder();
            enc.encode(w, vs[i]);
            if (!enc.complete) nested.set(i, enc);
          });
        },
        transmit: (path, tx) => transmitNested(nested, path, tx),
        get complete() {
          return nested.size === 0;
        },
      };
    },
    decoder() {
      /** @type {Map<number, Decoder>} */
      let nested = new Map();
      return {
        *decode(r) {
          nested = new Map();
          const vs = [];
          for (let i = 0; i < codecs.length; i++) {
            const dec = codecs[i].decoder();
            vs.push(yield* dec.decode(r));
            if (!dec.complete) nested.set(i, dec);
          }
          return join(vs);
        },
        receive: (path, at, data) => route(nested, path, at, data),
        get complete() {
          return nested.size === 0;
        },
      };
    },
  };
}

/** @param {Codec[]} codecs */
const tuple = (codecs) =>
  product(
    codecs,
    (v) => iterableToValues(v) ?? fail(new TypeError("expected an iterable for a `tuple`")),
    (vs) => vs,
  );

/** @param {Extract<Type, { kind: "record" }>} ty */
const record = (ty) =>
  product(
    ty.fields.map((f) => codec(f.ty)),
    (v) => {
      if ((typeof v !== "object" || v === null) && typeof v !== "function") {
        throw new TypeError("expected an object for a `record`");
      }
      return ty.fields.map((f) => v[f.key]);
    },
    (vs) => {
      /** @type {Record<string, any>} */
      const out = Object.create(null);
      ty.fields.forEach((f, i) => (out[f.key] = vs[i]));
      return out;
    },
  );

/** @param {Error} err @returns {never} */
function fail(err) {
  throw err;
}

/** @param {Codec | null} payload @returns {Codec} */
function future(payload) {
  return {
    encoder() {
      /** @type {any} */
      let value;
      let pending = false;
      return {
        encode(w, v) {
          w.u8(0);
          value = v;
          pending = true;
        },
        async transmit(path, tx) {
          const v = await (typeof value === "function" ? value() : value);
          if (!payload) {
            await tx.close(path);
            pending = false;
            return;
          }
          const enc = payload.encoder();
          const w = new Writer();
          enc.encode(w, v);
          await Promise.all([tx.write(path, w.finish()), tx.close(path)]);
          pending = false;
          if (!enc.complete) await enc.transmit([...path, 0], tx);
        },
        get complete() {
          return !pending;
        },
      };
    },
    decoder() {
      /** @type {((v: any) => void) | undefined} */
      let resolve;
      /** @type {Decoder | undefined} */
      let dec;
      /** @type {Feed | undefined} */
      let feed;
      let closed = true;
      return {
        *decode(r) {
          dec = payload?.decoder();
          resolve = undefined;
          if (!(yield* atomic(r, isPending))) {
            closed = true;
            return Promise.resolve(dec && (yield* dec.decode(r)));
          }
          closed = false;
          const d = dec;
          feed = d && new Feed((r) => d.decode(r));
          return new Promise((res) => (resolve = res));
        },
        receive(path, at, data) {
          if (at < path.length) {
            if (path[at] !== 0 || resolve || !dec) throw errorUnknownPath(path);
            return dec.receive(path, at + 1, data);
          }
          if (!resolve) {
            if (data.length || closed) throw errorDataAfterEnd();
            closed = true;
            return;
          }
          if (!data.length) {
            closed = true;
            if (feed && !feed.push(EMPTY)) throw errorEof("future closed without a value");
          } else {
            if (!feed) throw errorDataAfterEnd();
            if (!feed.push(data)) return;
            if (feed.rest().length) throw errorDataAfterEnd();
          }
          resolve(feed?.value);
          resolve = undefined;
          feed = undefined;
        },
        get complete() {
          return closed && (!dec || dec.complete);
        },
      };
    },
  };
}

/**
 * The encoding of `list<elem>` and of `stream<elem>` chunks.
 * @param {Codec} elem
 */
function items(elem) {
  const ty = elem.sync;
  const typed = ty && typedArray(ty);
  return {
    /**
     * Encodes `v`, returning the encoders of the pending values nested in its elements.
     * @param {Writer} w @param {ArrayLike<any>} v @returns {Map<number, Encoder>}
     */
    encode(w, v) {
      /** @type {Map<number, Encoder>} */
      const nested = new Map();
      if (ty) {
        encodeValue(w, { kind: "list", elem: ty }, v);
        return nested;
      }
      w.varU(v.length);
      for (let i = 0; i < v.length; i++) {
        const enc = elem.encoder();
        enc.encode(w, v[i]);
        if (!enc.complete) nested.set(i, enc);
      }
      return nested;
    },
    /**
     * Decodes `n` elements, returning them and the decoders of the pending values nested in them.
     * @param {Reader} r @param {number} n
     * @returns {Generator<number, [any[] | TypedArray, Map<number, Decoder>], void>}
     */
    *decode(r, n) {
      /** @type {Map<number, Decoder>} */
      const nested = new Map();
      if (ty) return [yield* readChunk(r, ty, n), nested];
      const vs = [];
      for (let i = 0; i < n; i++) {
        const dec = elem.decoder();
        vs.push(yield* dec.decode(r));
        if (!dec.complete) nested.set(i, dec);
      }
      return [vs, nested];
    },
    /** @param {(any[] | TypedArray)[]} parts */
    join(parts) {
      if (parts.length === 1) return parts[0];
      if (!typed) return parts.flat();
      const out = new typed(parts.reduce((n, part) => n + part.length, 0));
      let off = 0;
      for (const part of parts) {
        out.set(/** @type {any} */ (part), off);
        off += part.length;
      }
      return out;
    },
    /** @param {any} v */
    toList: (v) => (ty ? toList(v, ty) : (iterableToValues(v) ?? fail(listError()))),
  };
}

const listError = () => new TypeError("expected an iterable for a `list`");

/**
 * The chunks of the `stream<elem>` source `src`: any iterable or async
 * iterable of chunks, or a single typed array of a numeric `elem`.
 * @param {any} src @param {Type | undefined} elem
 * @returns {AsyncIterable<any> | Iterable<any>}
 */
function chunks(src, elem) {
  const typed = elem && typedArray(elem);
  if (typed && isTypedArray(src, typed)) return [src];
  if (
    typeof src?.[Symbol.asyncIterator] !== "function" &&
    typeof src?.[Symbol.iterator] !== "function"
  ) {
    throw new TypeError("expected an iterable or async iterable of chunks for a `stream`");
  }
  return src;
}

/** @param {Codec} elem @returns {Codec} */
function stream(elem) {
  const list = items(elem);
  return {
    encoder() {
      /** @type {any} */
      let src;
      let pending = false;
      return {
        encode(w, v) {
          src = chunks(v, elem.sync);
          w.u8(0);
          pending = true;
        },
        async transmit(path, tx) {
          /** @type {Set<Promise<void>>} */
          const nested = new Set();
          let closed = false;
          let stopped = false;
          /** @type {() => void} */
          let done = () => {};
          /** @type {(err: unknown) => void} */
          let failed = () => {};
          const all = new Promise((resolve, reject) => {
            done = () => resolve(undefined);
            failed = (err) => {
              stopped = true;
              reject(err);
            };
          });
          const settle = () => {
            if (closed && !nested.size) done();
          };
          const pump = async () => {
            let offset = 0;
            for await (const chunk of src) {
              if (stopped) return;
              const v = list.toList(chunk);
              if (!v.length) continue;
              const w = new Writer();
              const encs = list.encode(w, v);
              const written = tx.write(path, w.finish());
              for (const [i, enc] of encs) {
                const p = enc.transmit([...path, offset + i], tx).then(() => {
                  nested.delete(p);
                  settle();
                }, failed);
                nested.add(p);
              }
              offset += v.length;
              await written;
            }
            await tx.close(path);
            pending = false;
            closed = true;
            settle();
          };
          await Promise.all([pump(), all]);
        },
        get complete() {
          return !pending;
        },
      };
    },
    decoder() {
      /** @type {ReadableStreamDefaultController<any> | undefined} */
      let ctrl;
      let cancelled = false;
      /** @type {(() => void) | undefined} */
      let wake;
      /** @type {Map<number, Decoder>} */
      let nested = new Map();
      let offset = 0;
      /** @type {Feed | undefined} */
      let feed;
      /** @param {[any[] | TypedArray, Map<number, Decoder>]} chunk */
      const take = ([vs, decs]) => {
        for (const [i, dec] of decs) nested.set(offset + i, dec);
        offset += vs.length;
        return vs;
      };
      /** @param {Reader} r */
      function* chunk(r) {
        return take(yield* list.decode(r, yield* atomic(r, readVarU32)));
      }
      return {
        *decode(r) {
          nested = new Map();
          offset = 0;
          ctrl = undefined;
          feed = undefined;
          if (!(yield* atomic(r, isPending))) {
            const vs = yield* chunk(r);
            return new ReadableStream({
              start(c) {
                if (vs.length) c.enqueue(vs);
                c.close();
              },
            });
          }
          cancelled = false;
          return new ReadableStream(
            {
              start: (c) => void (ctrl = c),
              pull: () => wake?.(),
              cancel: () => {
                cancelled = true;
                wake?.();
              },
            },
            { highWaterMark: 1 },
          );
        },
        async receive(path, at, data) {
          if (at < path.length) return route(nested, path, at, data);
          if (!ctrl) throw errorDataAfterEnd();
          if (!data.length) {
            if (feed) throw errorEof("stream closed within a chunk");
            if (!cancelled) ctrl.close();
            ctrl = undefined;
            feed = undefined;
            return;
          }
          const c = ctrl;
          if (!cancelled && /** @type {number} */ (c.desiredSize) <= 0) {
            await new Promise((resolve) => (wake = () => resolve(undefined)));
            wake = undefined;
          }
          if (cancelled) throw new UnreadError();
          /** @type {(any[] | TypedArray)[]} */
          const parts = [];
          for (let rest = data; rest.length;) {
            feed ??= new Feed(chunk);
            if (!feed.push(rest)) break;
            if (feed.value.length) parts.push(feed.value);
            rest = feed.rest();
            feed = undefined;
          }
          if (parts.length) c.enqueue(list.join(parts));
        },
        get complete() {
          return !ctrl && nested.size === 0;
        },
      };
    },
  };
}

/**
 * The codec of the values of `ty`.
 * @param {Type} ty @returns {Codec}
 */
function codec(ty) {
  if (!hasAsync(ty)) return sync(ty);
  switch (ty.kind) {
    case "tuple":
      return tuple(ty.elems.map(codec));
    case "record":
      return record(ty);
    case "future":
      return future(ty.some && codec(ty.some));
    case "stream":
      return stream(codec(ty.elem));
    default:
      throw new TypeError(`pending values nested in \`${ty.kind}\` are not supported`);
  }
}

/** The encoder of the values of `ty`. @param {Type} ty */
export const encoder = (ty) => codec(ty).encoder();

/** The decoder of the values of `ty`. @param {Type} ty */
export const decoder = (ty) => codec(ty).decoder();

/**
 * Sends `value` on the root path using `enc`, then the pending values nested
 * in it. Resolves once all paths are closed.
 * @param {Encoder} enc @param {FrameWriter} tx @param {any} value
 */
export async function transmit(enc, tx, value) {
  const w = new Writer();
  enc.encode(w, value);
  await Promise.all([tx.write([], w.finish()), tx.close([])]);
  if (!enc.complete) await enc.transmit([], tx);
}

/**
 * Receives a value from the root path using `dec`, returning it and the
 * promise receiving the pending values nested in it, which resolves once all
 * paths are closed.
 * @param {Decoder} dec @param {FrameReader} rx
 * @returns {Promise<[any, Promise<void>]>}
 */
export async function receive(dec, rx) {
  const feed = new Feed((r) => dec.decode(r));
  let decoded = false;
  for (;;) {
    const data = await rx.next();
    if (data === null) throw errorEof("ingress ended before the root path was closed");
    if (rx.path.length) {
      if (!decoded) throw errorUnknownPath(rx.path);
      return [feed.value, receipt(dec, rx, false, data)];
    }
    if (!decoded) {
      decoded = feed.push(data);
      if (decoded && feed.rest().length) throw errorDataAfterEnd();
    } else if (data.length) {
      throw errorDataAfterEnd();
    }
    if (!data.length) {
      if (!decoded) throw errorEof("path closed without a value");
      return [feed.value, receipt(dec, rx, true, undefined)];
    }
  }
}

/**
 * @param {Decoder} dec @param {FrameReader} rx @param {boolean} closed
 * @param {Uint8Array | undefined} data
 */
async function receipt(dec, rx, closed, data) {
  for (;;) {
    if (data) await dec.receive(rx.path, 0, data);
    if (closed && dec.complete) return;
    const next = await rx.next();
    if (next === null) throw errorEof("ingress ended before all paths were closed");
    if (rx.path.length) {
      data = next;
    } else if (closed || next.length) {
      throw errorDataAfterEnd();
    } else {
      closed = true;
      data = undefined;
    }
  }
}
