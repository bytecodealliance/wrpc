// The component-model value codec, in the wRPC wire format. A value is encoded
// or decoded against a `Type` from `types.js`. This mirrors the canonical Rust
// codec in `wrpc-transport`'s `value.rs`.
//
// JavaScript representation of values (aligned with the component-model JS
// conventions used by `jco`):
//
//   bool                -> boolean
//   s8/u8/.../s32/u32   -> number
//   s64/u64             -> bigint
//   f32/f64             -> number
//   char                -> string (a single Unicode scalar)
//   string              -> string
//   list<u8>            -> Uint8Array
//   list<T>             -> Array
//   tuple               -> Array
//   record              -> object keyed by field name
//   option<T>           -> T | undefined (none is `undefined`/`null`), or
//                          { tag: "some", val } | { tag: "none" } if T is an option
//   result<O, E>        -> { tag: "ok", val } | { tag: "err", val }
//   variant             -> { tag: caseName, val? }
//   enum                -> string (the case name)
//   flags               -> { [name]: boolean }
//   own<R> / borrow<R>  -> Uint8Array (the opaque handle bytes)

import {
  Chan,
  Reader,
  UNEXPECTED_EOF,
  Writer,
  atomic,
  isUnexpectedEof,
  readVarU32,
  textEncoder,
  textDecoder,
} from "./io.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./index.d.ts").Value} Value */

/** The number of received chunks a pending `stream` buffers. */
const STREAM_CAPACITY = 128;

/** @param {Type} ty */
const isBytes = (ty) => ty.kind === "list" && ty.elem.kind === "u8";

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

/** @param {string} kind @param {string} [op] */
const asyncRequired = (kind, op = "decode") =>
  `\`${kind}\` values require the async session API (\`invoke\`/\`accept\`), ` +
  `not the synchronous \`${op}\``;

/**
 * Convert the integer `number` or `bigint` `v` to a `bigint` and assert it is
 * within `[min, max]`, rejecting fractional, non-numeric, or out-of-range
 * inputs instead of silently coercing or wrapping them on the wire.
 * @param {any} v @param {string} kind @param {bigint} min @param {bigint} max
 * @returns {bigint}
 */
function checkInt(v, kind, min, max) {
  let b;
  if (typeof v === "bigint") {
    b = v;
  } else {
    if (!Number.isInteger(v)) throw new TypeError(`expected an integer for \`${kind}\`, got ${v}`);
    b = BigInt(v);
  }
  if (b < min || b > max) {
    throw new RangeError(`\`${kind}\` value ${b} out of range [${min}, ${max}]`);
  }
  return b;
}

/** @param {any} v @param {string} kind @returns {number} */
function checkFloat(v, kind) {
  if (typeof v !== "number") throw new TypeError(`expected a number for \`${kind}\`, got ${v}`);
  return v;
}

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
 * `stream<u8>` chunks are returned as a `Uint8Array`; all others as an array.
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
  for (;;) {
    let pos = r.pos;
    try {
      for (; out.length < n; pos = r.pos) out.push(decodeScalar(r, elem));
      return out;
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
 * @param {{ path: number[], kind: "stream" | "future", ty: Type, value: any }[]} [sink]
 * @param {number[]} [path] relative index path to `v` (only tracked with `sink`)
 */
export function encodeValue(w, ty, v, sink, path) {
  switch (ty.kind) {
    case "bool":
      if (typeof v !== "boolean") throw new TypeError(`expected a boolean for a \`bool\`, got ${v}`);
      w.u8(v ? 1 : 0);
      return;
    case "u8":
      w.u8(Number(checkInt(v, "u8", 0n, 0xffn)));
      return;
    case "s8":
      w.u8(Number(checkInt(v, "s8", -0x80n, 0x7fn)) & 0xff);
      return;
    case "u16":
      w.varU(checkInt(v, "u16", 0n, 0xffffn));
      return;
    case "u32":
      w.varU(checkInt(v, "u32", 0n, 0xffffffffn));
      return;
    case "u64":
      w.varU(checkInt(v, "u64", 0n, (1n << 64n) - 1n));
      return;
    case "s16":
      w.varS(checkInt(v, "s16", -0x8000n, 0x7fffn));
      return;
    case "s32":
      w.varS(checkInt(v, "s32", -0x80000000n, 0x7fffffffn));
      return;
    case "s64":
      w.varS(checkInt(v, "s64", -(1n << 63n), (1n << 63n) - 1n));
      return;
    case "f32": {
      const view = new DataView(new ArrayBuffer(4));
      const n = checkFloat(v, "f32");
      if (Number.isNaN(n)) view.setUint32(0, 0x7fc00000, true);
      else view.setFloat32(0, n, true);
      w.bytes(new Uint8Array(view.buffer));
      return;
    }
    case "f64": {
      const view = new DataView(new ArrayBuffer(8));
      const n = checkFloat(v, "f64");
      if (Number.isNaN(n)) view.setBigUint64(0, 0x7ff8000000000000n, true);
      else view.setFloat64(0, n, true);
      w.bytes(new Uint8Array(view.buffer));
      return;
    }
    case "char": {
      const cp = typeof v === "string" ? v.codePointAt(0) : undefined;
      if (
        cp === undefined ||
        v.length !== (cp > 0xffff ? 2 : 1) ||
        (cp >= 0xd800 && cp <= 0xdfff)
      ) {
        throw new TypeError(`expected a single Unicode scalar value for a \`char\`, got ${v}`);
      }
      w.bytes(textEncoder.encode(v));
      return;
    }
    case "string":
      if (typeof v !== "string") throw new TypeError(`expected a string for a \`string\`, got ${v}`);
      if (/\p{Surrogate}/u.test(v)) throw new TypeError("a `string` cannot contain lone surrogates");
      w.name(v);
      return;
    case "list": {
      if (isBytes(ty)) {
        const bytes = toBytes(v);
        w.varU(bytes.length);
        w.bytes(bytes);
        return;
      }
      if (!Array.isArray(v)) throw new TypeError("expected an array for a `list`");
      w.varU(v.length);
      v.forEach((x, i) => encodeValue(w, ty.elem, x, sink, childPath(sink, path, i)));
      return;
    }
    case "tuple": {
      if (!Array.isArray(v) || v.length !== ty.elems.length) {
        throw new TypeError(`expected a ${ty.elems.length}-element tuple`);
      }
      ty.elems.forEach((et, i) => encodeValue(w, et, v[i], sink, childPath(sink, path, i)));
      return;
    }
    case "record": {
      if (typeof v !== "object" || v === null) throw new TypeError("expected an object for a `record`");
      ty.fields.forEach(({ name, ty: fty }, i) => {
        if (!(name in v)) throw new TypeError(`missing record field \`${name}\``);
        encodeValue(w, fty, v[name], sink, childPath(sink, path, i));
      });
      return;
    }
    case "option": {
      const { tag, val } = asOption(ty, v);
      if (tag === "none") {
        w.u8(0);
      } else {
        w.u8(1);
        encodeValue(w, ty.some, val, sink, path);
      }
      return;
    }
    case "result": {
      const { tag, val } = asResult(v);
      if (tag === "ok") {
        w.u8(0);
        if (ty.ok) encodeValue(w, ty.ok, val, sink, path);
      } else {
        w.u8(1);
        if (ty.err) encodeValue(w, ty.err, val, sink, path);
      }
      return;
    }
    case "enum": {
      const i = ty.names.indexOf(v);
      if (i < 0) throw new RangeError(`unknown enum case \`${v}\``);
      w.varU(i);
      return;
    }
    case "variant": {
      const { tag, val } = asVariant(v);
      const i = ty.cases.findIndex((c) => c.name === tag);
      if (i < 0) throw new RangeError(`unknown variant case \`${tag}\``);
      w.varU(i);
      if (ty.cases[i].ty) encodeValue(w, ty.cases[i].ty, val, sink, path);
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
      const set = toFlagSet(v);
      for (const name of set) {
        if (!ty.names.includes(name)) throw new RangeError(`unknown flag \`${name}\``);
      }
      const bytes = new Uint8Array(Math.ceil(Math.max(ty.names.length, 1) / 8));
      ty.names.forEach((name, i) => {
        if (set.has(name)) bytes[i >> 3] |= 1 << (i & 7);
      });
      w.bytes(bytes);
      return;
    }
    case "own":
    case "borrow": {
      const bytes = toBytes(v);
      w.varU(bytes.length);
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
      const out = {};
      for (let i = 0; i < ty.fields.length; i++) {
        const { name, ty: fty } = ty.fields[i];
        out[name] = yield* decodeSteps(r, fty, sink, childPath(sink, path, i));
      }
      return out;
    }
    case "option": {
      const nested = ty.some.kind === "option";
      if ((yield* atomic(r, (r) => r.u8())) === 0) return nested ? { tag: "none" } : undefined;
      const val = yield* decodeSteps(r, ty.some, sink, path);
      return nested ? { tag: "some", val } : val;
    }
    case "result": {
      const err = (yield* atomic(r, (r) => r.u8())) !== 0;
      const vty = err ? ty.err : ty.ok;
      return {
        tag: err ? "err" : "ok",
        val: vty ? yield* decodeSteps(r, vty, sink, path) : undefined,
      };
    }
    case "variant": {
      const i = yield* atomic(r, readVarU32);
      const c = ty.cases[i];
      if (!c) throw new RangeError(`unknown variant discriminant ${i}`);
      return c.ty ? { tag: c.name, val: yield* decodeSteps(r, c.ty, sink, path) } : { tag: c.name };
    }
    case "future": {
      const ready = (yield* atomic(r, (r) => r.u8())) !== 0;
      if (ready) return Promise.resolve(yield* decodeSteps(r, ty.some, sink, path));
      if (!sink) throw new Error(asyncRequired(ty.kind));
      /** @type {(value: any) => void} */
      let resolve = () => {};
      /** @type {(err: unknown) => void} */
      let reject = () => {};
      const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
      });
      // The failure also surfaces through `done`, so an ignored future must not be unhandled.
      promise.catch(() => {});
      sink.push({ path: path ?? [], kind: "future", ty: ty.some, resolve, reject });
      return promise;
    }
    case "stream": {
      const n = yield* atomic(r, readVarU32);
      if (n > 0) {
        // A stream sent inline as a single ready chunk.
        const before = sink?.length ?? 0;
        const chunk = yield* readChunk(r, ty.elem, n, sink, path);
        const q = new Chan();
        q.push(chunk);
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
      return r.u8() !== 0;
    case "u8":
      return r.u8();
    case "s8": {
      const b = r.u8();
      return b >= 0x80 ? b - 0x100 : b;
    }
    case "u16":
      return Number(r.varU(16));
    case "u32":
      return Number(r.varU(32));
    case "s16":
      return Number(r.varS(16));
    case "s32":
      return Number(r.varS(32));
    case "u64":
      return r.varU(64);
    case "s64":
      return r.varS(64);
    case "f32":
      return new DataView(r.take(4).slice().buffer).getFloat32(0, true);
    case "f64":
      return new DataView(r.take(8).slice().buffer).getFloat64(0, true);
    case "char": {
      // Read one UTF-8 scalar value: the lead byte determines the length.
      const lead = r.peek();
      const len = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4;
      return textDecoder.decode(r.take(len));
    }
    case "string":
      return r.name();
    case "enum": {
      const i = Number(r.varU(32));
      const name = ty.names[i];
      if (name === undefined) throw new RangeError(`unknown enum discriminant ${i}`);
      return name;
    }
    case "flags": {
      const bytes = r.take(Math.ceil(Math.max(ty.names.length, 1) / 8));
      /** @type {Record<string, boolean>} */
      const out = {};
      ty.names.forEach((name, i) => {
        out[name] = (bytes[i >> 3] & (1 << (i & 7))) !== 0;
      });
      return out;
    }
    case "own":
    case "borrow":
      return r.take(Number(r.varU(32))).slice();
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

/** @param {any} v @returns {Uint8Array} */
export function toBytes(v) {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  if (Array.isArray(v)) return Uint8Array.from(v, (b) => Number(checkInt(b, "u8", 0n, 0xffn)));
  throw new TypeError("expected `Uint8Array`, `ArrayBuffer`, or `number[]` for a byte list");
}

/**
 * An `option` is `T | undefined`, unless `T` is itself an `option`: then the
 * outer one is tagged, so that `some(none)` is distinct from `none`.
 * @param {{ some: Type }} ty @param {any} v
 */
function asOption(ty, v) {
  if (ty.some.kind !== "option") {
    return v === null || v === undefined ? { tag: "none" } : { tag: "some", val: v };
  }
  if (v && typeof v === "object" && (v.tag === "some" || v.tag === "none")) {
    return { tag: v.tag, val: v.val };
  }
  throw new TypeError('expected `{ tag: "some", val } | { tag: "none" }` for a nested `option`');
}

/** @param {any} v */
function asResult(v) {
  if (v && typeof v === "object" && (v.tag === "ok" || v.tag === "err")) {
    return { tag: v.tag, val: v.val };
  }
  throw new TypeError('expected `{ tag: "ok" | "err", val }` for a `result`');
}

/** @param {any} v */
function asVariant(v) {
  if (typeof v === "string") return { tag: v, val: undefined };
  if (v && typeof v === "object" && typeof v.tag === "string") return { tag: v.tag, val: v.val };
  throw new TypeError("expected a string or `{ tag, val }` for a `variant`");
}

/** @param {any} v */
function toFlagSet(v) {
  if (v instanceof Set) return v;
  if (Array.isArray(v)) return new Set(v);
  if (v && typeof v === "object") {
    return new Set(Object.keys(v).filter((k) => v[k]));
  }
  throw new TypeError("expected an object, array, or `Set` for `flags`");
}
