// The codecs of the component-model values without nested values, in the
// wRPC wire format, and the conversions of JavaScript values, which the codec
// tree of `codec.js` builds on. This mirrors the canonical Rust codec in
// `wrpc-core`'s `value.rs`.
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
//   list<T, N>          -> as list<T>
//   map<K, V>           -> Map
//   tuple               -> Array
//   record              -> null-prototype object keyed by camel case field name
//   option<T>           -> T | undefined (none is `undefined`/`null`), or
//                          { kind: "some", value } | { kind: "none" } if T is an option
//   result<O, E>        -> { kind: "ok", value? } | { kind: "error", value? }
//   variant             -> { kind: caseName, value? }
//   enum                -> string (the case name)
//   flags               -> null-prototype object of booleans keyed by camel case name
//   own<R> / borrow<R>  -> Uint8Array (the opaque handle bytes)

import { atomic, isUnexpectedEof, readVarInt, readVarU32, textEncoder, textDecoder } from "./io.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./io.js").Reader} Reader */
/** @typedef {import("./io.js").Writer} Writer */

/** @typedef {import("./index.d.ts").TypedArray} TypedArray */
/**
 * @typedef {{ name: string, new (length: number): TypedArray, from(elems: ArrayLike<any>, map?: (e: any) => any): TypedArray }}
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
 * The inclusive bounds of each integer type.
 * @type {Record<string, [number, number] | [bigint, bigint]>}
 */
const INT_BOUNDS = {
  s8: [-0x80, 0x7f],
  u8: [0, 0xff],
  s16: [-0x8000, 0x7fff],
  u16: [0, 0xffff],
  s32: [-0x80000000, 0x7fffffff],
  u32: [0, 0xffffffff],
  s64: [-(1n << 63n), (1n << 63n) - 1n],
  u64: [0n, (1n << 64n) - 1n],
};

/**
 * `v` if it is an integer of the integer type `kind`, which is a `number`, or
 * a `bigint` for `s64` and `u64`.
 * @param {string} kind @param {unknown} v @returns {number | bigint}
 */
function checkInt(kind, v) {
  const [min, max] = INT_BOUNDS[kind];
  if (typeof v !== typeof min) {
    throw new TypeError(`expected a ${typeof min} for a \`${kind}\`, got ${typeof v}`);
  }
  const n = /** @type {number | bigint} */ (v);
  if ((typeof n === "number" && !Number.isInteger(n)) || n < min || n > max) {
    throw new RangeError(`expected an integer in [${min}, ${max}] for a \`${kind}\`, got ${n}`);
  }
  return n;
}

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
 * Read a status byte, rejecting any but `0` and `1` like the Rust codec.
 * @param {string} what @returns {(r: Reader) => boolean} whether it is `1`
 */
const status = (what) => (r) => {
  const b = r.u8();
  if (b > 1) throw new RangeError(`invalid ${what} byte \`${b}\``);
  return b === 1;
};
export const optionStatus = status("option status");
export const resultStatus = status("result status");
const boolStatus = status("bool value");

/** Scratch space to convert floats to and from their little-endian bytes. */
const floatView = new DataView(new ArrayBuffer(8));
const floatBytes = new Uint8Array(floatView.buffer);
const f32Bytes = floatBytes.subarray(0, 4);

/**
 * Decode `n` elements of the type `elem` without nested values from `r`,
 * as the typed array of a numeric `elem`, or an array.
 * @param {Reader} r @param {Type} elem @param {number} n
 * @returns {Generator<number, any, void>}
 */
export function* readChunk(r, elem, n) {
  if (elem.kind === "u8") return yield* atomic(r, (r) => r.take(n).slice());
  const out = [];
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
 * Encode a value of a type without nested values into `w`.
 * @param {Writer} w
 * @param {Type} ty
 * @param {any} v
 */
export function encodeScalar(w, ty, v) {
  switch (ty.kind) {
    case "bool":
      w.u8(v ? 1 : 0);
      return;
    case "u8":
    case "s8":
      w.u8(/** @type {number} */ (checkInt(ty.kind, v)) & 0xff);
      return;
    case "u16":
    case "u32":
    case "u64":
      w.varU(checkInt(ty.kind, v));
      return;
    case "s16":
    case "s32":
    case "s64":
      w.varS(checkInt(ty.kind, v));
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
    case "enum": {
      const s = `${v}`;
      const i = ty.names.indexOf(s);
      if (i < 0) throw new TypeError(`unknown enum case \`${s}\``);
      w.varU(i);
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
    case "own":
    case "borrow": {
      const bytes = /** @type {Uint8Array} */ (toList(v, u8));
      w.varU(bytes.length);
      w.bytes(bytes);
      return;
    }
    default:
      throw new Error(`encoding \`${/** @type {any} */ (ty).kind}\` is not supported`);
  }
}

/**
 * Decode a value of a type without nested values from `r`.
 * @param {Reader} r
 * @param {Type} ty
 * @returns {any}
 */
export function decodeScalar(r, ty) {
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

/** @param {unknown} v @returns {v is Record<string, any>} */
export const isObject = (v) => (typeof v === "object" && v !== null) || typeof v === "function";

/**
 * The values `v` iterates, or `undefined` if `v` is not an iterable object.
 * @param {unknown} v @returns {unknown[] | undefined}
 */
export function iterableToValues(v) {
  if (!isObject(v)) return undefined;
  const iter = /** @type {any} */ (v)[Symbol.iterator];
  if (iter === undefined || iter === null) return undefined;
  return [...{ [Symbol.iterator]: () => iter.call(v) }];
}

/**
 * The entries of `v`, or `undefined` if `v` is not a `Map` from any realm.
 * @param {unknown} v @returns {[unknown, unknown][] | undefined}
 */
export function mapEntries(v) {
  try {
    return [...Map.prototype.entries.call(v)];
  } catch {
    return undefined;
  }
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
  if (!typed) return elems;
  if (!Object.hasOwn(INT_BOUNDS, elem.kind)) return typed.from(elems);
  return typed.from(elems, (e) => checkInt(elem.kind, e));
}

const u8 = /** @type {Type} */ ({ kind: "u8" });

export const OPTION_CASES = ["none", "some"];
export const RESULT_CASES = ["ok", "error"];

/**
 * A null-prototype `{ kind, value? }` object, with `value` only if given.
 * @param {string} kind @param {...any} value
 */
export function tagged(kind, ...value) {
  const out = Object.create(null);
  out.kind = kind;
  if (value.length) out.value = value[0];
  return out;
}

/**
 * The index of the case of `cases` that the `{ kind, value? }` object `v` is.
 * @param {any} v @param {string[]} cases @param {string} what @returns {number}
 */
export function caseIndex(v, cases, what) {
  if (!isObject(v)) throw new TypeError(`expected \`{ kind, value? }\` for ${what}`);
  const kind = `${v.kind}`;
  const i = cases.indexOf(kind);
  if (i < 0) throw new TypeError(`unknown case \`${kind}\` of ${what}`);
  return i;
}
