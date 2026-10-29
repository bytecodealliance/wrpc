// The component-model value codec, in the wRPC wire format. A value is encoded
// or decoded against a `Type` from `types.js`. This mirrors the canonical Rust
// codec in `wrpc-transport`'s `value.rs`.
//
// JavaScript representation of values (following the ToJSValue and
// ToComponentValue conversions of the component model's JS-API draft, see
// `README.md` for where it deviates):
//
//   bool                -> boolean
//   s8/u8/.../s32/u32   -> number
//   s64/u64             -> bigint
//   f32/f64             -> number
//   char                -> string (a single Unicode scalar)
//   string              -> string
//   list<s8>/list<u8>   -> Int8Array/Uint8Array
//   list<s16>/list<u16> -> Int16Array/Uint16Array
//   list<s32>/list<u32> -> Int32Array/Uint32Array
//   list<s64>/list<u64> -> BigInt64Array/BigUint64Array
//   list<f32>/list<f64> -> Float32Array/Float64Array
//   list<T>             -> Array
//   tuple               -> Array
//   record              -> null-prototype object keyed by camel case field name
//   option<T>           -> T | undefined (none is `undefined`/`null`), or
//                          { kind: "some", value } | { kind: "none" } if T is an option
//   result<O, E>        -> { kind: "ok", value? } | { kind: "error", value? }
//   variant             -> { kind: caseName, value? }
//   enum                -> string (the case name)
//   flags               -> null-prototype object of booleans keyed by camel case name
//   own<R> / borrow<R>  -> Uint8Array (the opaque handle bytes)

import {
  Chan,
  UNEXPECTED_EOF,
  atomic,
  isUnexpectedEof,
  readVarInt,
  readVarU32,
  textEncoder,
  textDecoder,
} from "./io.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./index.d.ts").Value} Value */
/** @typedef {import("./io.js").Reader} Reader */
/** @typedef {import("./io.js").Writer} Writer */

/** The number of received chunks a pending `stream` buffers. */
const STREAM_CAPACITY = 128;

/** @typedef {import("./index.d.ts").TypedArray} TypedArray */
/**
 * @typedef {{ name: string, new (length: number): TypedArray, from(elems: ArrayLike<any>): TypedArray }}
 *   TypedArrayConstructor
 */

/**
 * The typed arrays that lists of each numeric type map to.
 * @type {Record<string, TypedArrayConstructor>}
 */
const TYPED_ARRAYS = {
  s8: Int8Array,
  u8: Uint8Array,
  s16: Int16Array,
  u16: Uint16Array,
  s32: Int32Array,
  u32: Uint32Array,
  s64: BigInt64Array,
  u64: BigUint64Array,
  f32: Float32Array,
  f64: Float64Array,
};

/**
 * The typed array that a list of `elem` maps to, if `elem` is numeric.
 * @param {Type} elem @returns {TypedArrayConstructor | undefined}
 */
export const typedArray = (elem) =>
  Object.hasOwn(TYPED_ARRAYS, elem.kind) ? TYPED_ARRAYS[elem.kind] : undefined;

/** The name of the typed array `this` is, from any realm, or `undefined`. */
const typedArrayName = /** @type {(this: unknown) => string | undefined} */ (
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Int8Array.prototype), Symbol.toStringTag)
    ?.get
);

/**
 * Whether `v` is a `Typed` typed array, from any realm.
 * @template {TypedArrayConstructor} T
 * @param {unknown} v @param {T} Typed @returns {v is InstanceType<T>}
 */
export const isTypedArray = (v, Typed) => typedArrayName.call(v) === Typed.name;

/**
 * Extend a relative index `path` by structural index `i`, but only while a
 * `sink` is being collected (otherwise paths are irrelevant and left
 * `undefined`). `option`/`result`/`variant` keep the parent's path; `list`,
 * `tuple` and `record` push the element/field index, matching the wRPC
 * indexing rules.
 * @param {unknown} sink
 * @param {number[] | undefined} path
 * @param {number} i
 */
const childPath = (sink, path, i) => (sink ? [...(path ?? []), i] : undefined);

/**
 * Read a status byte, rejecting any but `0` and `1` like the Rust codec.
 * @param {string} what @returns {(r: Reader) => boolean} whether it is `1`
 */
const status = (what) => (r) => {
  const b = r.u8();
  if (b > 1) throw new RangeError(`invalid ${what} byte \`${b}\``);
  return b === 1;
};
const optionStatus = status("option status");
const resultStatus = status("result status");
const boolStatus = status("bool value");

/** @param {string} kind @param {string} [op] */
const asyncRequired = (kind, op = "decode") =>
  `\`${kind}\` values require the async session API (\`invoke\`/\`accept\`), ` +
  `not the synchronous \`${op}\``;

/** Scratch space to convert floats to and from their little-endian bytes. */
const floatView = new DataView(new ArrayBuffer(8));
const floatBytes = new Uint8Array(floatView.buffer);
const f32Bytes = floatBytes.subarray(0, 4);

/** The kinds of the types whose values contain other values. */
const COMPOSITE = new Set([
  "list",
  "tuple",
  "record",
  "option",
  "result",
  "variant",
  "future",
  "stream",
]);

/**
 * Decode a single stream/list chunk of `n` elements of type `elem` from `r`.
 * Chunks of numeric types are returned as the matching typed array, all
 * others as an array.
 * @param {Reader} r
 * @param {Type} elem
 * @param {number} n
 * @param {import("./index.d.ts").DecodeDeferred[]} [sink]
 * @param {number[]} [path]
 * @returns {Generator<number, any, void>}
 */
export function* readChunk(r, elem, n, sink, path) {
  if (elem.kind === "u8") return yield* atomic(r, (r) => r.take(n).slice());
  const out = [];
  if (COMPOSITE.has(elem.kind)) {
    for (let i = 0; i < n; i++) {
      out.push(yield* decodeSteps(r, elem, sink, childPath(sink, path, i)));
    }
    return out;
  }
  // Resume from the last whole element rather than one generator step per element.
  const typed = typedArray(elem);
  for (;;) {
    let pos = r.pos;
    try {
      for (; out.length < n; pos = r.pos) out.push(decodeScalar(r, elem));
      return typed ? typed.from(out) : out;
    } catch (err) {
      if (!isUnexpectedEof(err)) throw err;
      r.pos = pos;
      yield r.need - pos;
    }
  }
}

/**
 * Encode a value against a type into `w`.
 *
 * `stream` and `future` values cannot be written synchronously: their data is
 * multiplexed over the invocation's framed stream after the synchronous header.
 * When `sink` is supplied (by the async session machinery) the pending marker
 * is written and a descriptor `{ path, kind, ty, value }` is appended to it,
 * recording the relative `path` at which the deferred data must be transmitted.
 * Without a `sink`, encountering one throws.
 *
 * @param {Writer} w
 * @param {Type} ty
 * @param {any} v
 * @param {{ path: number[], kind: "stream" | "future", ty: Type | null, value: any }[]} [sink]
 * @param {number[]} [path] relative index path to `v` (only tracked with `sink`)
 */
export function encodeValue(w, ty, v, sink, path) {
  switch (ty.kind) {
    case "bool":
      w.u8(v ? 1 : 0);
      return;
    case "u8":
    case "s8":
      w.u8(+v & 0xff);
      return;
    case "u16":
      w.varU(+v & 0xffff);
      return;
    case "u32":
      w.varU(+v >>> 0);
      return;
    case "u64":
      w.varU(BigInt.asUintN(64, v));
      return;
    case "s16":
      w.varS((+v << 16) >> 16);
      return;
    case "s32":
      w.varS(+v | 0);
      return;
    case "s64":
      w.varS(BigInt.asIntN(64, v));
      return;
    case "f32": {
      const n = +v;
      if (Number.isNaN(n)) floatView.setUint32(0, 0x7fc00000, true);
      else floatView.setFloat32(0, n, true);
      w.bytes(f32Bytes);
      return;
    }
    case "f64": {
      const n = +v;
      if (Number.isNaN(n)) floatView.setBigUint64(0, 0x7ff8000000000000n, true);
      else floatView.setFloat64(0, n, true);
      w.bytes(floatBytes);
      return;
    }
    case "char": {
      const s = `${v}`;
      const cp = s.codePointAt(0);
      if (
        cp === undefined ||
        s.length !== (cp > 0xffff ? 2 : 1) ||
        (cp >= 0xd800 && cp <= 0xdfff)
      ) {
        throw new TypeError(`expected a single Unicode scalar value for a \`char\`, got ${s}`);
      }
      w.bytes(textEncoder.encode(s));
      return;
    }
    case "string":
      w.name(`${v}`);
      return;
    case "list":
    case "own":
    case "borrow": {
      const elem = ty.kind === "list" ? ty.elem : u8;
      const elems = toList(v, elem);
      w.varU(elems.length);
      if (elem.kind === "u8") {
        w.bytes(/** @type {Uint8Array} */ (elems));
        return;
      }
      for (let i = 0; i < elems.length; i++)
        encodeValue(w, elem, elems[i], sink, childPath(sink, path, i));
      return;
    }
    case "tuple": {
      const elems = iterableToValues(v);
      if (!elems) throw new TypeError(`expected an iterable for a \`tuple\``);
      ty.elems.forEach((et, i) => encodeValue(w, et, elems[i], sink, childPath(sink, path, i)));
      return;
    }
    case "record": {
      if (!isObject(v)) throw new TypeError("expected an object for a `record`");
      ty.fields.forEach(({ key, ty: fty }, i) =>
        encodeValue(w, fty, v[key], sink, childPath(sink, path, i)),
      );
      return;
    }
    case "option": {
      if (ty.some.kind === "option") {
        const i = caseIndex(v, OPTION_CASES, "a nested `option`");
        w.u8(i);
        if (i) encodeValue(w, ty.some, v.value, sink, path);
      } else if (v === null || v === undefined) {
        w.u8(0);
      } else {
        w.u8(1);
        encodeValue(w, ty.some, v, sink, path);
      }
      return;
    }
    case "result": {
      const i = caseIndex(v, RESULT_CASES, "a `result`");
      w.u8(i);
      const pty = i ? ty.err : ty.ok;
      if (pty) encodeValue(w, pty, v.value, sink, path);
      return;
    }
    case "enum": {
      const s = `${v}`;
      const i = ty.names.indexOf(s);
      if (i < 0) throw new TypeError(`unknown enum case \`${s}\``);
      w.varU(i);
      return;
    }
    case "variant": {
      const i = caseIndex(
        v,
        ty.cases.map((c) => c.name),
        "a `variant`",
      );
      w.varU(i);
      const pty = ty.cases[i].ty;
      if (pty) encodeValue(w, pty, v.value, sink, path);
      return;
    }
    case "future":
    case "stream": {
      // Mark the value pending; its data flows over the framed sub-stream.
      w.u8(0x00);
      if (!sink) throw new Error(asyncRequired(ty.kind, "encode"));
      const elem = ty.kind === "future" ? ty.some : ty.elem;
      sink.push({ path: path ?? [], kind: ty.kind, ty: elem, value: v });
      return;
    }
    case "flags": {
      if (!isObject(v)) throw new TypeError("expected an object for `flags`");
      const bytes = new Uint8Array(Math.ceil(ty.flags.length / 8));
      ty.flags.forEach(({ key }, i) => {
        if (v[key]) bytes[i >> 3] |= 1 << (i & 7);
      });
      w.bytes(bytes);
      return;
    }
    default:
      throw new Error(`encoding \`${/** @type {any} */ (ty).kind}\` is not supported`);
  }
}

/**
 * Decode a value of type `ty` from `r`.
 *
 * `stream` and `future` values carry their data asynchronously over the
 * framed sub-streams. When `sink` is supplied (by the async session
 * machinery), a placeholder is returned — a `Promise` for a `future`, an async
 * iterable of chunks for a `stream` — and a descriptor recording the relative
 * `path` and the placeholder's resolver/queue is appended to `sink`. Without a
 * `sink`, encountering one throws. A `stream` sent inline as a single ready
 * chunk is decoded either way, and recorded in `sink` as `inline` if its
 * elements carry pending values.
 *
 * @param {Reader} r
 * @param {Type} ty
 * @param {import("./index.d.ts").DecodeDeferred[]} [sink]
 * @param {number[]} [path] relative index path to this value (only with `sink`)
 * @returns {any}
 */
export function decodeValue(r, ty, sink, path) {
  const step = decodeSteps(r, ty, sink, path).next();
  if (!step.done) throw new RangeError(UNEXPECTED_EOF);
  return step.value;
}

/**
 * {@link decodeValue} as a resumable decoder: when `r` runs out, it yields the
 * number of bytes it needs from `r.pos` on, and continues once `r` holds them.
 * @param {Reader} r
 * @param {Type} ty
 * @param {import("./index.d.ts").DecodeDeferred[]} [sink]
 * @param {number[]} [path]
 * @returns {Generator<number, any, void>}
 */
export function* decodeSteps(r, ty, sink, path) {
  switch (ty.kind) {
    case "list":
      return yield* readChunk(r, ty.elem, yield* atomic(r, readVarU32), sink, path);
    case "tuple": {
      const out = [];
      for (let i = 0; i < ty.elems.length; i++) {
        out.push(yield* decodeSteps(r, ty.elems[i], sink, childPath(sink, path, i)));
      }
      return out;
    }
    case "record": {
      /** @type {Record<string, any>} */
      const out = Object.create(null);
      for (let i = 0; i < ty.fields.length; i++) {
        const { key, ty: fty } = ty.fields[i];
        out[key] = yield* decodeSteps(r, fty, sink, childPath(sink, path, i));
      }
      return out;
    }
    case "option": {
      const nested = ty.some.kind === "option";
      if (!(yield* atomic(r, optionStatus))) return nested ? tagged("none") : undefined;
      const value = yield* decodeSteps(r, ty.some, sink, path);
      return nested ? tagged("some", value) : value;
    }
    case "result": {
      const err = yield* atomic(r, resultStatus);
      const kind = RESULT_CASES[err ? 1 : 0];
      const vty = err ? ty.err : ty.ok;
      return vty ? tagged(kind, yield* decodeSteps(r, vty, sink, path)) : tagged(kind);
    }
    case "variant": {
      const i = yield* atomic(r, readVarU32);
      const c = ty.cases[i];
      if (!c) throw new RangeError(`unknown variant discriminant ${i}`);
      return c.ty ? tagged(c.name, yield* decodeSteps(r, c.ty, sink, path)) : tagged(c.name);
    }
    case "future": {
      if (yield* atomic(r, optionStatus)) {
        return Promise.resolve(ty.some ? yield* decodeSteps(r, ty.some, sink, path) : undefined);
      }
      if (!sink) throw new Error(asyncRequired(ty.kind));
      /** @type {(value: any) => void} */
      let resolve = () => {};
      const promise = new Promise((res) => (resolve = res));
      sink.push({ path: path ?? [], kind: "future", ty: ty.some, resolve });
      return promise;
    }
    case "stream": {
      if (yield* atomic(r, optionStatus)) {
        // A stream sent inline as a single ready chunk.
        const n = yield* atomic(r, readVarU32);
        const before = sink?.length ?? 0;
        const chunk = yield* readChunk(r, ty.elem, n, sink, path);
        const q = new Chan();
        if (n > 0) q.push(chunk);
        q.close();
        if (sink && sink.length > before) sink.push({ path: path ?? [], kind: "inline", queue: q });
        return q;
      }
      if (!sink) throw new Error(asyncRequired(ty.kind));
      const queue = new Chan(STREAM_CAPACITY);
      sink.push({ path: path ?? [], kind: "stream", ty: ty.elem, queue });
      return queue;
    }
    default:
      return yield* atomic(r, (r) => decodeScalar(r, ty));
  }
}

/**
 * Decode a value of a type that is not {@link COMPOSITE} from `r`.
 * @param {Reader} r
 * @param {Type} ty
 * @returns {any}
 */
function decodeScalar(r, ty) {
  switch (ty.kind) {
    case "bool":
      return boolStatus(r);
    case "u8":
      return r.u8();
    case "s8": {
      const b = r.u8();
      return b >= 0x80 ? b - 0x100 : b;
    }
    case "u16":
      return readVarInt(r, 16, false);
    case "u32":
      return readVarU32(r);
    case "s16":
      return readVarInt(r, 16, true);
    case "s32":
      return readVarInt(r, 32, true);
    case "u64":
      return r.varU(64);
    case "s64":
      return r.varS(64);
    case "f32":
      floatBytes.set(r.take(4));
      return floatView.getFloat32(0, true);
    case "f64":
      floatBytes.set(r.take(8));
      return floatView.getFloat64(0, true);
    case "char": {
      // Read one UTF-8 scalar value: the lead byte determines the length.
      const lead = r.peek();
      const len = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4;
      return textDecoder.decode(r.take(len));
    }
    case "string":
      return r.name();
    case "enum": {
      const i = readVarU32(r);
      const name = ty.names[i];
      if (name === undefined) throw new RangeError(`unknown enum discriminant ${i}`);
      return name;
    }
    case "flags": {
      const n = ty.flags.length;
      const bytes = r.take(Math.ceil(n / 8));
      if (bytes[bytes.length - 1] >> (((n - 1) % 8) + 1))
        throw new RangeError("invalid flags bits");
      /** @type {Record<string, boolean>} */
      const out = Object.create(null);
      ty.flags.forEach(({ key }, i) => {
        out[key] = (bytes[i >> 3] & (1 << (i & 7))) !== 0;
      });
      return out;
    }
    case "own":
    case "borrow":
      return r.take(readVarU32(r)).slice();
    default:
      throw new Error(`decoding \`${/** @type {any} */ (ty).kind}\` is not supported`);
  }
}

/**
 * Whether values of `ty` can carry pending `stream`/`future` values.
 * @param {Type} ty @returns {boolean}
 */
export function hasAsync(ty) {
  switch (ty.kind) {
    case "future":
    case "stream":
      return true;
    case "list":
      return hasAsync(ty.elem);
    case "option":
      return hasAsync(ty.some);
    case "result":
      return (!!ty.ok && hasAsync(ty.ok)) || (!!ty.err && hasAsync(ty.err));
    case "tuple":
      return ty.elems.some(hasAsync);
    case "record":
      return ty.fields.some((f) => hasAsync(f.ty));
    case "variant":
      return ty.cases.some((c) => !!c.ty && hasAsync(c.ty));
    default:
      return false;
  }
}

/** @param {unknown} v @returns {v is Record<string, any>} */
const isObject = (v) => (typeof v === "object" && v !== null) || typeof v === "function";

/**
 * The values `v` iterates, or `undefined` if `v` is not an iterable object.
 * @param {unknown} v @returns {unknown[] | undefined}
 */
function iterableToValues(v) {
  if (!isObject(v)) return undefined;
  const iter = /** @type {any} */ (v)[Symbol.iterator];
  if (iter === undefined || iter === null) return undefined;
  return [...{ [Symbol.iterator]: () => iter.call(v) }];
}

/**
 * The elements of the `list<elem>` value or `stream<elem>` chunk `v`: the
 * typed array of a numeric `elem`, copying any other iterable into one, or the
 * values `v` iterates.
 * @param {unknown} v @param {Type} elem @returns {ArrayLike<any>}
 */
export function toList(v, elem) {
  const typed = typedArray(elem);
  if (typed && isTypedArray(v, typed)) {
    if (/** @type {any} */ (v.buffer).detached)
      throw new TypeError("expected a typed array with an attached buffer");
    return v;
  }
  const elems = iterableToValues(v);
  if (!elems) throw new TypeError(`expected an iterable for a \`list<${elem.kind}>\``);
  return typed ? typed.from(elems) : elems;
}

const u8 = /** @type {Type} */ ({ kind: "u8" });
const OPTION_CASES = ["none", "some"];
const RESULT_CASES = ["ok", "error"];

/**
 * A null-prototype `{ kind, value? }` object, with `value` only if given.
 * @param {string} kind @param {...any} value
 */
function tagged(kind, ...value) {
  const out = Object.create(null);
  out.kind = kind;
  if (value.length) out.value = value[0];
  return out;
}

/**
 * The index of the case of `cases` that the `{ kind, value? }` object `v` is.
 * @param {any} v @param {string[]} cases @param {string} what @returns {number}
 */
function caseIndex(v, cases, what) {
  if (!isObject(v)) throw new TypeError(`expected \`{ kind, value? }\` for ${what}`);
  const kind = `${v.kind}`;
  const i = cases.indexOf(kind);
  if (i < 0) throw new TypeError(`unknown case \`${kind}\` of ${what}`);
  return i;
}
