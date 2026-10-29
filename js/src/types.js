// Constructors for the type tree the value codec walks. A `Type` mirrors a WIT
// type structurally:
//
//   { kind: "bool" | "s8" | "u8" | "s16" | "u16" | "s32" | "u32" | "s64"
//          | "u64" | "f32" | "f64" | "char" | "string" }
//   | { kind: "list", elem: Type }
//   | { kind: "option", some: Type }
//   | { kind: "result", ok: Type | null, err: Type | null }
//   | { kind: "tuple", elems: Type[] }
//   | { kind: "record", fields: { name: string, key: string, ty: Type }[] }
//   | { kind: "variant", cases: { name: string, ty: Type | null }[] }
//   | { kind: "enum", names: string[] }
//   | { kind: "flags", flags: { name: string, key: string }[] }
//   | { kind: "own" | "borrow", name: string }
//
// This is the structural type tree the value codec and session walk. Names are
// the WIT names; the `key` of a record field or flag is the camel case
// property name its value has in JavaScript, as in the component model's
// JS-API draft.

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
 * `future<some>`. Carries a single asynchronous value, multiplexed over the
 * invocation's framed stream. Requires the async session API (`invoke` /
 * `accept`); the synchronous `encode`/`decode` cannot transmit it. Pass no
 * type for a bare `future` without a payload.
 * @param {Type | null} [some]
 */
export const future = (some = null) => ({ kind: "future", some });

/**
 * `stream<elem>`. Carries an asynchronous sequence of `elem` chunks,
 * multiplexed over the invocation's framed stream. Requires the async session
 * API (`invoke` / `accept`).
 * @param {Type} elem
 */
export const stream = (elem) => ({ kind: "stream", elem });

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
 * `{ name, ty }`, keyed by the WIT field names. Field order is the wire order
 * and must match the WIT.
 * @param {Record<string, Type> | { name: string, ty: Type }[]} fields
 */
export const record = (fields) => ({
  kind: "record",
  fields: uniqueKeys(normalizeFields(fields), "record field"),
});

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

/** Flags, by their WIT names. @param {string[]} names */
export const flags = (names) => ({
  kind: "flags",
  flags: uniqueKeys(
    names.map((name) => ({ name, key: camelCase(name) })),
    "flag",
  ),
});

/** An owned resource handle `own<name>`. @param {string} name */
export const own = (name) => ({ kind: "own", name });

/** A borrowed resource handle `borrow<name>`. @param {string} name */
export const borrow = (name) => ({ kind: "borrow", name });

/**
 * `items`, unless two of them have the same key: then it throws.
 * @template {{ name: string, key: string }} T
 * @param {T[]} items @param {string} what @returns {T[]}
 */
function uniqueKeys(items, what) {
  /** @type {Map<string, string>} */
  const names = new Map();
  for (const { name, key } of items) {
    const other = names.get(key);
    if (other !== undefined) {
      throw new TypeError(`${what}s \`${other}\` and \`${name}\` both have the key \`${key}\``);
    }
    names.set(key, name);
  }
  return items;
}

/** @param {Record<string, Type> | { name: string, ty: Type }[]} fields */
function normalizeFields(fields) {
  const entries = Array.isArray(fields)
    ? fields.map(({ name, ty }) => /** @type {const} */ ([name, ty]))
    : Object.entries(fields);
  return entries.map(([name, ty]) => ({ name, key: camelCase(name), ty }));
}

/**
 * The camel case form of the WIT label `name`, as the component model's
 * JS-API draft names record fields and flags: its first fragment lowercased,
 * followed by the others capitalized, unless they are acronyms, e.g.
 * `favorite-color` is `favoriteColor` and `inner-HTML` is `innerHTML`.
 * @param {string} name
 */
function camelCase(name) {
  const [first, ...rest] = name.split("-");
  return (
    first.toLowerCase() +
    rest.map((f) => (/^[0-9A-Z]+$/.test(f) ? f : f.charAt(0).toUpperCase() + f.slice(1))).join("")
  );
}

/** @param {Record<string, Type | null> | { name: string, ty: Type | null }[]} cases */
function normalizeCases(cases) {
  if (Array.isArray(cases)) return cases.map(({ name, ty }) => ({ name, ty: ty ?? null }));
  return Object.entries(cases).map(([name, ty]) => ({ name, ty: ty ?? null }));
}

// `enum` is a reserved word, so the constructor is exported as `enumType`; this
// alias lets callers write `t.enum([...])` via the namespace import.
export { enumType as enum };
