// The codec tree: the encoders and decoders of the values of a `Type`, which
// transmit and receive the pending values nested in the last value they
// encoded or decoded over the frames of an invocation (see `SPEC.md`,
// "Framed stream specification" and "Indexing").

import { Reader, UNEXPECTED_EOF, Writer, atomic, readVarU32 } from "./io.js";
import {
  OPTION_CASES,
  RESULT_CASES,
  caseIndex,
  decodeScalar,
  encodeScalar,
  isObject,
  isTypedArray,
  iterableToValues,
  mapEntries,
  optionStatus,
  readChunk,
  resultStatus,
  tagged,
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
 * @typedef {{ encoder(): Encoder, decoder(): Decoder, leaf?: Type }} Codec
 * @typedef {ArrayLike<any> | number} Chunk
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
const isPending = (r) => !optionStatus(r);

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

/** @param {readonly number[]} path @returns {never} */
const unknownPath = (path) => {
  throw errorUnknownPath(path);
};

/** @param {Error} err @returns {never} */
function fail(err) {
  throw err;
}

/**
 * The codec of the values of a type without nested values.
 * @param {Type} ty @returns {Codec}
 */
function leaf(ty) {
  /** @type {Encoder} */
  const enc = {
    encode: (w, v) => encodeScalar(w, ty, v),
    transmit: async () => {},
    complete: true,
  };
  /** @type {Decoder} */
  const dec = {
    decode: (r) => atomic(r, (r) => decodeScalar(r, ty)),
    receive: unknownPath,
    complete: true,
  };
  return { encoder: () => enc, decoder: () => dec, leaf: ty };
}

/**
 * The codec of values, which keep the pending values nested in them indexed.
 * @param {(w: Writer, v: any) => Map<number, Encoder>} encode
 * @param {(r: Reader) => Generator<number, [any, Map<number, Decoder>], void>} decode
 * @returns {Codec}
 */
function indexed(encode, decode) {
  return {
    encoder() {
      /** @type {Map<number, Encoder>} */
      let nested = new Map();
      return {
        encode(w, v) {
          nested = encode(w, v);
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
          const [v, decs] = yield* decode(r);
          nested = decs;
          return v;
        },
        receive: (path, at, data) => route(nested, path, at, data),
        get complete() {
          return nested.size === 0;
        },
      };
    },
  };
}

/**
 * The codec of values of a case, whose payload's pending values are not indexed.
 * @param {(w: Writer, v: any) => [Codec | null, any]} encode writes the case
 * @param {(r: Reader) => Generator<number, [Codec | null, (v?: any) => any], void>} decode
 *   reads the case
 * @returns {Codec}
 */
function cases(encode, decode) {
  return {
    encoder() {
      /** @type {Encoder | undefined} */
      let payload;
      return {
        encode(w, v) {
          const [c, pv] = encode(w, v);
          payload = c?.encoder();
          payload?.encode(w, pv);
        },
        transmit: async (path, tx) => payload?.transmit(path, tx),
        get complete() {
          return !payload || payload.complete;
        },
      };
    },
    decoder() {
      /** @type {Decoder | undefined} */
      let payload;
      return {
        *decode(r) {
          const [c, make] = yield* decode(r);
          payload = c?.decoder();
          return payload ? make(yield* payload.decode(r)) : make();
        },
        receive(path, at, data) {
          if (!payload) throw errorUnknownPath(path);
          return payload.receive(path, at, data);
        },
        get complete() {
          return !payload || payload.complete;
        },
      };
    },
  };
}

/**
 * Encodes the values `vs` using `codecs` into `w`, returning the encoders of the pending values
 * nested in them.
 * @param {Writer} w @param {Codec[]} codecs @param {ArrayLike<any>} vs
 */
function encodeAll(w, codecs, vs) {
  /** @type {Map<number, Encoder>} */
  const nested = new Map();
  codecs.forEach((c, i) => {
    const enc = c.encoder();
    enc.encode(w, vs[i]);
    if (!enc.complete) nested.set(i, enc);
  });
  return nested;
}

/**
 * Decodes values using `codecs` from `r`, returning them and the decoders of the pending values
 * nested in them.
 * @param {Reader} r @param {Codec[]} codecs
 * @returns {Generator<number, [any[], Map<number, Decoder>], void>}
 */
function* decodeAll(r, codecs) {
  /** @type {Map<number, Decoder>} */
  const nested = new Map();
  const vs = [];
  for (let i = 0; i < codecs.length; i++) {
    const dec = codecs[i].decoder();
    vs.push(yield* dec.decode(r));
    if (!dec.complete) nested.set(i, dec);
  }
  return [vs, nested];
}

/** @param {Codec[]} codecs */
const tuple = (codecs) =>
  indexed(
    (w, v) =>
      encodeAll(
        w,
        codecs,
        iterableToValues(v) ?? fail(new TypeError("expected an iterable for a `tuple`")),
      ),
    (r) => decodeAll(r, codecs),
  );

/** @param {Extract<Type, { kind: "record" }>} ty */
function record(ty) {
  const codecs = ty.fields.map((f) => codec(f.ty));
  return indexed(
    (w, v) => {
      if (!isObject(v)) throw new TypeError("expected an object for a `record`");
      return encodeAll(
        w,
        codecs,
        ty.fields.map((f) => v[f.key]),
      );
    },
    function* (r) {
      const [vs, nested] = yield* decodeAll(r, codecs);
      /** @type {Record<string, any>} */
      const out = Object.create(null);
      ty.fields.forEach((f, i) => (out[f.key] = vs[i]));
      return [out, nested];
    },
  );
}

/**
 * The elements of `list<elem>` values and `stream<elem>` chunks, or the
 * numbers of elements of the chunks of a `stream` without an element type.
 * @param {Codec | null} elem
 */
function items(elem) {
  const ty = elem?.leaf;
  const typed = ty && typedArray(ty);
  return {
    /** @param {any} v @returns {Chunk} */
    from(v) {
      if (!elem) {
        if (!Number.isInteger(v) || v < 0 || v > 0xffffffff) {
          throw new TypeError("expected a number of elements for a `stream` chunk");
        }
        return v;
      }
      return ty
        ? toList(v, ty)
        : (iterableToValues(v) ?? fail(new TypeError("expected an iterable for a `list`")));
    },
    /** @param {Chunk} v */
    count: (v) => (typeof v === "number" ? v : v.length),
    /**
     * Encodes the elements of `v`, returning the encoders of the pending values nested in them.
     * @param {Writer} w @param {Chunk} v @returns {Map<number, Encoder>}
     */
    encode(w, v) {
      /** @type {Map<number, Encoder>} */
      const nested = new Map();
      if (typeof v === "number" || !elem) return nested;
      if (ty?.kind === "u8") {
        w.bytes(/** @type {Uint8Array} */ (v));
        return nested;
      }
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
     * @returns {Generator<number, [Chunk, Map<number, Decoder>], void>}
     */
    *decode(r, n) {
      /** @type {Map<number, Decoder>} */
      const nested = new Map();
      if (!elem) return [n, nested];
      if (ty) return [yield* readChunk(r, ty, n), nested];
      const vs = [];
      for (let i = 0; i < n; i++) {
        const dec = elem.decoder();
        vs.push(yield* dec.decode(r));
        if (!dec.complete) nested.set(i, dec);
      }
      return [vs, nested];
    },
    /** @param {Chunk[]} parts @returns {Chunk} */
    join(parts) {
      if (parts.length === 1) return parts[0];
      if (!elem) return /** @type {number[]} */ (parts).reduce((a, b) => a + b);
      if (!typed) return /** @type {any[][]} */ (parts).flat();
      const typedParts = /** @type {TypedArray[]} */ (parts);
      const out = new typed(typedParts.reduce((n, part) => n + part.length, 0));
      let off = 0;
      for (const part of typedParts) {
        out.set(/** @type {any} */ (part), off);
        off += part.length;
      }
      return out;
    },
  };
}

/** @param {Codec} elem @param {number | undefined} length */
function list(elem, length) {
  const its = items(elem);
  return indexed(
    (w, v) => {
      const vs = its.from(v);
      const n = its.count(vs);
      if (length === undefined) w.varU(n);
      else if (n !== length) {
        throw new RangeError(`expected ${length} elements for a fixed-length list, got ${n}`);
      }
      return its.encode(w, vs);
    },
    function* (r) {
      return yield* its.decode(r, length ?? (yield* atomic(r, readVarU32)));
    },
  );
}

/** @param {Codec} key @param {Codec} value */
function map(key, value) {
  const its = items(tuple([key, value]));
  return indexed(
    (w, v) => {
      const entries = mapEntries(v) ?? fail(new TypeError("expected a `Map` for a `map`"));
      w.varU(entries.length);
      return its.encode(w, entries);
    },
    function* (r) {
      const n = yield* atomic(r, readVarU32);
      const [entries, nested] = yield* its.decode(r, n);
      const m = new Map(/** @type {[any, any][]} */ (entries));
      if (m.size !== n) throw new RangeError("duplicate map key");
      return [m, nested];
    },
  );
}

/** @param {Extract<Type, { kind: "option" }>} ty */
function option(ty) {
  const some = codec(ty.some);
  const nested = ty.some.kind === "option";
  return cases(
    (w, v) => {
      if (nested) {
        const i = caseIndex(v, OPTION_CASES, "a nested `option`");
        w.u8(i);
        return i ? [some, v.value] : [null, undefined];
      }
      if (v === null || v === undefined) {
        w.u8(0);
        return [null, undefined];
      }
      w.u8(1);
      return [some, v];
    },
    function* (r) {
      if (!(yield* atomic(r, optionStatus))) {
        return [null, () => (nested ? tagged("none") : undefined)];
      }
      return [some, (v) => (nested ? tagged("some", v) : v)];
    },
  );
}

/** @param {Codec | null} ok @param {Codec | null} err */
function result(ok, err) {
  return cases(
    (w, v) => {
      const i = caseIndex(v, RESULT_CASES, "a `result`");
      w.u8(i);
      return [i ? err : ok, v.value];
    },
    function* (r) {
      const i = (yield* atomic(r, resultStatus)) ? 1 : 0;
      return [i ? err : ok, (...v) => tagged(RESULT_CASES[i], ...v)];
    },
  );
}

/** @param {Extract<Type, { kind: "variant" }>} ty */
function variant(ty) {
  const names = ty.cases.map((c) => c.name);
  const codecs = ty.cases.map((c) => c.ty && codec(c.ty));
  return cases(
    (w, v) => {
      const i = caseIndex(v, names, "a `variant`");
      w.varU(i);
      return [codecs[i], v.value];
    },
    function* (r) {
      const i = yield* atomic(r, readVarU32);
      if (i >= names.length) throw new RangeError(`unknown variant discriminant ${i}`);
      return [codecs[i], (...v) => tagged(names[i], ...v)];
    },
  );
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

/** @param {Codec | null} elem @returns {Codec} */
function stream(elem) {
  const list = items(elem);
  return {
    encoder() {
      /** @type {any} */
      let src;
      let pending = false;
      return {
        encode(w, v) {
          src = chunks(v, elem?.leaf);
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
              const v = list.from(chunk);
              const n = list.count(v);
              if (!n) continue;
              const w = new Writer();
              w.varU(n);
              const encs = list.encode(w, v);
              const written = tx.write(path, w.finish());
              for (const [i, enc] of encs) {
                const p = enc.transmit([...path, offset + i], tx).then(() => {
                  nested.delete(p);
                  settle();
                }, failed);
                nested.add(p);
              }
              offset += n;
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
      /** @param {[Chunk, Map<number, Decoder>]} chunk */
      const take = ([vs, decs]) => {
        for (const [i, dec] of decs) nested.set(offset + i, dec);
        offset += list.count(vs);
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
                if (list.count(vs)) c.enqueue(vs);
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
          /** @type {Chunk[]} */
          const parts = [];
          for (let rest = data; rest.length;) {
            feed ??= new Feed(chunk);
            if (!feed.push(rest)) break;
            if (list.count(feed.value)) parts.push(feed.value);
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
 * Whether the values of `ty` have an empty encoding.
 * @param {Type} ty @returns {boolean}
 */
function empty(ty) {
  switch (ty.kind) {
    case "tuple":
      return ty.elems.every(empty);
    case "record":
      return ty.fields.every((f) => empty(f.ty));
    case "list":
      return ty.length === 0 || (ty.length !== undefined && empty(ty.elem));
    default:
      return false;
  }
}

/**
 * The codec of the elements of type `ty` of a `list` or `stream`, which must
 * not have an empty encoding, since their numbers would not be bounded by the
 * data received.
 * @param {Type} ty
 */
function element(ty) {
  if (empty(ty)) {
    throw new TypeError("list and stream elements must not have an empty encoding");
  }
  return codec(ty);
}

/**
 * The codec of the values of `ty`.
 * @param {Type} ty @returns {Codec}
 */
function codec(ty) {
  switch (ty.kind) {
    case "list":
      return list(element(ty.elem), ty.length);
    case "map":
      return map(codec(ty.key), codec(ty.value));
    case "tuple":
      return tuple(ty.elems.map(codec));
    case "record":
      return record(ty);
    case "option":
      return option(ty);
    case "result":
      return result(ty.ok && codec(ty.ok), ty.err && codec(ty.err));
    case "variant":
      return variant(ty);
    case "future":
      return future(ty.some && codec(ty.some));
    case "stream":
      return stream(ty.elem && element(ty.elem));
    default:
      return leaf(ty);
  }
}

/**
 * Encodes `value` using `enc` into `w`, failing if any value nested in it is
 * pending, as `future` and `stream` values always are.
 * @param {Encoder} enc @param {any} value @param {Writer} w
 */
export function pack(enc, value, w) {
  enc.encode(w, value);
  if (!enc.complete) throw new TypeError("asynchronous value is pending");
}

/**
 * Decodes a value using `dec` from all of `r`, failing if any value nested in
 * it is pending.
 * @param {Decoder} dec @param {Reader} r
 */
export function unpack(dec, r) {
  const step = dec.decode(r).next();
  if (!step.done) throw errorEof("buffer is incomplete");
  if (!dec.complete) {
    throw new RangeError("buffer contains pending asynchronous values, which cannot be unpacked");
  }
  if (!r.done) throw new RangeError("buffer contains trailing bytes");
  return step.value;
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
