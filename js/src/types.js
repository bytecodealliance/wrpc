// Constructors for the type tree the value codec walks. A `Type` mirrors a WIT
// type structurally:
//
//   { kind: "bool" | "s8" | "u8" | "s16" | "u16" | "s32" | "u32" | "s64"
//          | "u64" | "f32" | "f64" | "char" | "string" }
//   | { kind: "list", elem: Type }
//   | { kind: "option", some: Type }
//   | { kind: "result", ok: Type | null, err: Type | null }
//   | { kind: "tuple", elems: Type[] }
//   | { kind: "record", fields: { name: string, ty: Type }[] }
//   | { kind: "variant", cases: { name: string, ty: Type | null }[] }
//   | { kind: "enum", names: string[] }
//   | { kind: "flags", names: string[] }
//   | { kind: "own" | "borrow", name: string }
//
// The same tree is produced by the WIT parser in `wit.js`.

export const bool = { kind: "bool" };
export const s8 = { kind: "s8" };
export const u8 = { kind: "u8" };
export const s16 = { kind: "s16" };
export const u16 = { kind: "u16" };
export const s32 = { kind: "s32" };
export const u32 = { kind: "u32" };
export const s64 = { kind: "s64" };
export const u64 = { kind: "u64" };
export const f32 = { kind: "f32" };
export const f64 = { kind: "f64" };
export const char = { kind: "char" };
export const string = { kind: "string" };

/** @typedef {import("./index.d.ts").Type} Type */

/** `list<elem>` @param {Type} elem */
export const list = (elem) => ({ kind: "list", elem });

/** `option<some>` @param {Type} some */
export const option = (some) => ({ kind: "option", some });

/**
 * `result<ok, err>`. Pass `null` for an absent payload, e.g. `result(null, e)`
 * is `result<_, e>` and `result()` is bare `result`.
 * @param {Type | null} [ok]
 * @param {Type | null} [err]
 */
export const result = (ok = null, err = null) => ({ kind: "result", ok, err });

/** `tuple<...elems>` @param {...Type} elems */
export const tuple = (...elems) => ({ kind: "tuple", elems });

/**
 * A record. Accepts an ordered field map `{ name: Type }` or an array of
 * `{ name, ty }`. Field order is the wire order and must match the WIT.
 * @param {Record<string, Type> | { name: string, ty: Type }[]} fields
 */
export const record = (fields) => ({ kind: "record", fields: normalizeFields(fields) });

/**
 * A variant. Accepts a case map `{ name: Type | null }` (use `null` for a
 * payload-less case) or an array of `{ name, ty }`.
 * @param {Record<string, Type | null> | { name: string, ty: Type | null }[]} cases
 */
export const variant = (cases) => ({
  kind: "variant",
  cases: normalizeCases(cases),
});

/** An enum. @param {string[]} names */
export const enumType = (names) => ({ kind: "enum", names: [...names] });

/** Flags. @param {string[]} names */
export const flags = (names) => ({ kind: "flags", names: [...names] });

/** An owned resource handle `own<name>`. @param {string} name */
export const own = (name) => ({ kind: "own", name });

/** A borrowed resource handle `borrow<name>`. @param {string} name */
export const borrow = (name) => ({ kind: "borrow", name });

/** @param {Record<string, Type> | { name: string, ty: Type }[]} fields */
function normalizeFields(fields) {
  if (Array.isArray(fields)) return fields.map(({ name, ty }) => ({ name, ty }));
  return Object.entries(fields).map(([name, ty]) => ({ name, ty }));
}

/** @param {Record<string, Type | null> | { name: string, ty: Type | null }[]} cases */
function normalizeCases(cases) {
  if (Array.isArray(cases)) return cases.map(({ name, ty }) => ({ name, ty: ty ?? null }));
  return Object.entries(cases).map(([name, ty]) => ({ name, ty: ty ?? null }));
}

// `enum` is a reserved word, so the constructor is exported as `enumType`; this
// alias lets callers write `t.enum([...])` via the namespace import.
export { enumType as enum };
