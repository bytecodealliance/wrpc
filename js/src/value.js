// The component-model value codec, in the wRPC wire format. A value is encoded
// or decoded against a `Type` from `types.js` (or the WIT parser). This mirrors
// the canonical Rust codec in `wrpc-transport`'s `value.rs`.
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

import { Reader, Writer, textEncoder, textDecoder } from "./io.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./index.d.ts").Value} Value */

/** @param {Type} ty */
const isBytes = (ty) => ty.kind === "list" && ty.elem.kind === "u8";

/**
 * Encode a value against a type into `w`.
 * @param {Writer} w
 * @param {Type} ty
 * @param {any} v
 */
export function encodeValue(w, ty, v) {
  switch (ty.kind) {
    case "bool":
      w.u8(v ? 1 : 0);
      return;
    case "u8":
    case "s8":
      w.u8(Number(v) & 0xff);
      return;
    case "u16":
    case "u32":
    case "u64":
      w.varU(typeof v === "bigint" ? v : Math.trunc(Number(v)));
      return;
    case "s16":
    case "s32":
    case "s64":
      w.varS(typeof v === "bigint" ? v : Math.trunc(Number(v)));
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
      for (const x of v) encodeValue(w, ty.elem, x);
      return;
    }
    case "tuple": {
      if (!Array.isArray(v) || v.length !== ty.elems.length) {
        throw new TypeError(`expected a ${ty.elems.length}-element tuple`);
      }
      ty.elems.forEach((et, i) => encodeValue(w, et, v[i]));
      return;
    }
    case "record": {
      if (typeof v !== "object" || v === null) throw new TypeError("expected an object for a `record`");
      for (const { name, ty: fty } of ty.fields) {
        if (!(name in v)) throw new TypeError(`missing record field \`${name}\``);
        encodeValue(w, fty, v[name]);
      }
      return;
    }
    case "option":
      if (v === null || v === undefined) {
        w.u8(0);
      } else {
        w.u8(1);
        encodeValue(w, ty.some, v);
      }
      return;
    case "result": {
      const { tag, val } = asResult(v);
      if (tag === "ok") {
        w.u8(0);
        if (ty.ok) encodeValue(w, ty.ok, val);
      } else {
        w.u8(1);
        if (ty.err) encodeValue(w, ty.err, val);
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
      if (ty.cases[i].ty) encodeValue(w, ty.cases[i].ty, val);
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
 * @param {Reader} r
 * @param {Type} ty
 * @returns {any}
 */
export function decodeValue(r, ty) {
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
      const lead = r.bytes[r.pos];
      const len = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4;
      return textDecoder.decode(r.take(len));
    }
    case "string":
      return r.name();
    case "list": {
      const n = Number(r.varU());
      if (isBytes(ty)) return r.take(n).slice();
      const out = new Array(n);
      for (let i = 0; i < n; i++) out[i] = decodeValue(r, ty.elem);
      return out;
    }
    case "tuple":
      return ty.elems.map((et) => decodeValue(r, et));
    case "record": {
      /** @type {Record<string, any>} */
      const out = {};
      for (const { name, ty: fty } of ty.fields) out[name] = decodeValue(r, fty);
      return out;
    }
    case "option":
      return r.u8() !== 0 ? decodeValue(r, ty.some) : undefined;
    case "result":
      return r.u8() !== 0
        ? { tag: "err", val: ty.err ? decodeValue(r, ty.err) : undefined }
        : { tag: "ok", val: ty.ok ? decodeValue(r, ty.ok) : undefined };
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
      return c.ty ? { tag: c.name, val: decodeValue(r, c.ty) } : { tag: c.name };
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

/**
 * Encode a single value to a fresh `Uint8Array`.
 * @param {Type} ty
 * @param {any} v
 */
export function encode(ty, v) {
  const w = new Writer();
  encodeValue(w, ty, v);
  return w.finish();
}

/**
 * Decode a single value from `bytes`.
 * @param {Type} ty
 * @param {Uint8Array} bytes
 */
export function decode(ty, bytes) {
  return decodeValue(new Reader(bytes), ty);
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
