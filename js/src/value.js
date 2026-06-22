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
//   option<T>           -> T | undefined (none is `undefined`/`null`)
//   result<O, E>        -> { tag: "ok", val } | { tag: "err", val }
//   variant             -> { tag: caseName, val? }
//   enum                -> string (the case name)
//   flags               -> { [name]: boolean }
//   own<R> / borrow<R>  -> Uint8Array (the opaque handle bytes)

import { Chan, Reader, Writer, textEncoder, textDecoder } from "./io.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./index.d.ts").Value} Value */

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
 * Coerce `v` to an integer `bigint` and assert it is within `[min, max]`,
 * rejecting fractional, non-numeric, or out-of-range inputs instead of
 * silently wrapping them on the wire.
 * @param {any} v @param {string} kind @param {bigint} min @param {bigint} max
 * @returns {bigint}
 */
function checkInt(v, kind, min, max) {
  let b;
  if (typeof v === "bigint") {
    b = v;
  } else {
    const n = Number(v);
    if (!Number.isInteger(n)) throw new TypeError(`expected an integer for \`${kind}\`, got ${v}`);
    b = BigInt(n);
  }
  if (b < min || b > max) {
    throw new RangeError(`\`${kind}\` value ${b} out of range [${min}, ${max}]`);
  }
  return b;
}

/**
 * Decode a single stream/list chunk of `n` elements of type `elem` from `r`.
 * `stream<u8>` chunks are returned as a `Uint8Array`; all others as an array.
 * @param {Reader} r
 * @param {Type} elem
 * @param {number} n
 * @param {import("./index.d.ts").DecodeDeferred[]} [sink]
 * @param {number[]} [path]
 */
function readChunk(r, elem, n, sink, path) {
  if (elem.kind === "u8") return r.take(n).slice();
  const out = new Array(n);
  for (let i = 0; i < n; i++) out[i] = decodeValue(r, elem, sink, childPath(sink, path, i));
  return out;
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
      const buf = new ArrayBuffer(4);
      new DataView(buf).setFloat32(0, Number(v), true);
      w.bytes(new Uint8Array(buf));
      return;
    }
    case "f64": {
      const buf = new ArrayBuffer(8);
      new DataView(buf).setFloat64(0, Number(v), true);
      w.bytes(new Uint8Array(buf));
      return;
    }
    case "char":
      w.bytes(textEncoder.encode(String(v)));
      return;
    case "string":
      w.name(String(v));
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
    case "option":
      if (v === null || v === undefined) {
        w.u8(0);
      } else {
        w.u8(1);
        encodeValue(w, ty.some, v, sink, path);
      }
      return;
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
      const i = ty.names.indexOf(String(v));
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
 * `sink`, encountering one throws.
 *
 * @param {Reader} r
 * @param {Type} ty
 * @param {import("./index.d.ts").DecodeDeferred[]} [sink]
 * @param {number[]} [path] relative index path to this value (only with `sink`)
 * @returns {any}
 */
export function decodeValue(r, ty, sink, path) {
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
    case "u32":
      return Number(r.varU());
    case "s16":
    case "s32":
      return Number(r.varS());
    case "u64":
      return r.varU();
    case "s64":
      return r.varS();
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
    case "list":
      return readChunk(r, ty.elem, Number(r.varU()), sink, path);
    case "tuple":
      return ty.elems.map((et, i) => decodeValue(r, et, sink, childPath(sink, path, i)));
    case "record": {
      /** @type {Record<string, any>} */
      const out = {};
      ty.fields.forEach(({ name, ty: fty }, i) => {
        out[name] = decodeValue(r, fty, sink, childPath(sink, path, i));
      });
      return out;
    }
    case "option":
      return r.u8() !== 0 ? decodeValue(r, ty.some, sink, path) : undefined;
    case "result":
      return r.u8() !== 0
        ? { tag: "err", val: ty.err ? decodeValue(r, ty.err, sink, path) : undefined }
        : { tag: "ok", val: ty.ok ? decodeValue(r, ty.ok, sink, path) : undefined };
    case "enum": {
      const i = Number(r.varU());
      const name = ty.names[i];
      if (name === undefined) throw new RangeError(`unknown enum discriminant ${i}`);
      return name;
    }
    case "variant": {
      const i = Number(r.varU());
      const c = ty.cases[i];
      if (!c) throw new RangeError(`unknown variant discriminant ${i}`);
      return c.ty ? { tag: c.name, val: decodeValue(r, c.ty, sink, path) } : { tag: c.name };
    }
    case "future": {
      const ready = r.u8() !== 0;
      if (ready) return Promise.resolve(decodeValue(r, ty.some, sink, path));
      if (!sink) throw new Error(asyncRequired(ty.kind));
      /** @type {(value: any) => void} */
      let resolve = () => {};
      const promise = new Promise((res) => {
        resolve = res;
      });
      sink.push({ path: path ?? [], kind: "future", ty: ty.some, resolve });
      return promise;
    }
    case "stream": {
      const n = Number(r.varU());
      if (n > 0) {
        // A stream sent inline as a single ready chunk.
        const chunk = readChunk(r, ty.elem, n, sink, path);
        const q = new Chan();
        q.push(chunk);
        q.close();
        return q;
      }
      if (!sink) throw new Error(asyncRequired(ty.kind));
      const queue = new Chan();
      sink.push({ path: path ?? [], kind: "stream", ty: ty.elem, queue });
      return queue;
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
      return r.take(Number(r.varU())).slice();
    default:
      throw new Error(`decoding \`${/** @type {any} */ (ty).kind}\` is not supported`);
  }
}

/** @param {any} v */
function toBytes(v) {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  if (Array.isArray(v)) return Uint8Array.from(v);
  throw new TypeError("expected `Uint8Array`, `ArrayBuffer`, or `number[]` for a byte list");
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
