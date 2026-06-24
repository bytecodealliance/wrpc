// Type definitions for the `wrpc` package.

/** A structural wRPC/WIT type. */
export type Type =
  | { kind: "bool" | "s8" | "u8" | "s16" | "u16" | "s32" | "u32" | "s64" | "u64" | "f32" | "f64" | "char" | "string" }
  | { kind: "list"; elem: Type }
  | { kind: "option"; some: Type }
  | { kind: "result"; ok: Type | null; err: Type | null }
  | { kind: "tuple"; elems: Type[] }
  | { kind: "record"; fields: { name: string; ty: Type }[] }
  | { kind: "variant"; cases: { name: string; ty: Type | null }[] }
  | { kind: "enum"; names: string[] }
  | { kind: "flags"; names: string[] }
  | { kind: "own" | "borrow"; name: string };

/** The JavaScript representation of a decoded component-model value. */
export type Value =
  | boolean
  | number
  | bigint
  | string
  | Uint8Array
  | Value[]
  | { [field: string]: Value }
  | { tag: string; val?: Value }
  | undefined;

// ----- types.js -------------------------------------------------------------

export namespace types {
  const bool: Type;
  const s8: Type;
  const u8: Type;
  const s16: Type;
  const u16: Type;
  const s32: Type;
  const u32: Type;
  const s64: Type;
  const u64: Type;
  const f32: Type;
  const f64: Type;
  const char: Type;
  const string: Type;
  function list(elem: Type): Type;
  function option(some: Type): Type;
  function result(ok?: Type | null, err?: Type | null): Type;
  function tuple(...elems: Type[]): Type;
  function record(fields: Record<string, Type> | { name: string; ty: Type }[]): Type;
  function variant(cases: Record<string, Type | null> | { name: string; ty: Type | null }[]): Type;
  function enumType(names: string[]): Type;
  export { enumType as enum };
  function flags(names: string[]): Type;
  function own(name: string): Type;
  function borrow(name: string): Type;
}

export { types as t };

// ----- io.js ----------------------------------------------------------------

export class Writer {
  constructor();
  u8(b: number): void;
  bytes(bytes: Uint8Array | number[]): void;
  varU(value: number | bigint): void;
  varS(value: number | bigint): void;
  name(str: string): void;
  finish(): Uint8Array;
}

export class Reader {
  constructor(bytes: Uint8Array);
  readonly done: boolean;
  bytes: Uint8Array;
  pos: number;
  u8(): number;
  take(n: number): Uint8Array;
  varU(): bigint;
  varS(): bigint;
  name(): string;
}

// ----- value.js -------------------------------------------------------------

export function encode(ty: Type, v: Value): Uint8Array;
export function decode(ty: Type, bytes: Uint8Array): Value;
export function encodeValue(w: Writer, ty: Type, v: Value): void;
export function decodeValue(r: Reader, ty: Type): Value;

// ----- wit.js ---------------------------------------------------------------

export function parseType(text: string): Type;
export function parseWit(text: string): {
  package: string | null;
  interface: string;
  funcs: { name: string; params: { name: string; ty: Type }[]; results: Type[] }[];
};

// ----- frame.js -------------------------------------------------------------

export const PROTOCOL: number;
export function encodeInvocation(instance: string, func: string, paramTypes: Type[], args: Value[]): Uint8Array;
export function decodeResults(bytes: Uint8Array, resultTypes: Type[]): Value[];
export function concatBytes(chunks: Uint8Array[]): Uint8Array;
