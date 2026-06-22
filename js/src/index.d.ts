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
  | { kind: "future"; some: Type }
  | { kind: "stream"; elem: Type }
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
  | Promise<Value>
  | AsyncIterable<Value[] | Uint8Array>
  | undefined;

/**
 * A descriptor recorded by {@link decodeValue} for a pending `stream`/`future`
 * value, naming the relative sub-stream `path` and the placeholder to feed.
 */
export type DecodeDeferred =
  | { path: number[]; kind: "future"; ty: Type; resolve: (value: Value) => void }
  | { path: number[]; kind: "stream"; ty: Type; queue: Chan<Value[] | Uint8Array> };

// ----- types.js -------------------------------------------------------------

export namespace types {
  export const bool: Type;
  export const s8: Type;
  export const u8: Type;
  export const s16: Type;
  export const u16: Type;
  export const s32: Type;
  export const u32: Type;
  export const s64: Type;
  export const u64: Type;
  export const f32: Type;
  export const f64: Type;
  export const char: Type;
  export const string: Type;
  export function list(elem: Type): Type;
  export function option(some: Type): Type;
  export function future(some: Type): Type;
  export function stream(elem: Type): Type;
  export function result(ok?: Type | null, err?: Type | null): Type;
  export function tuple(...elems: Type[]): Type;
  export function record(fields: Record<string, Type> | { name: string; ty: Type }[]): Type;
  export function variant(cases: Record<string, Type | null> | { name: string; ty: Type | null }[]): Type;
  function enumType(names: string[]): Type;
  export { enumType as enum };
  export function flags(names: string[]): Type;
  export function own(name: string): Type;
  export function borrow(name: string): Type;
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
  peek(): number;
  take(n: number): Uint8Array;
  varU(): bigint;
  varS(): bigint;
  name(): string;
}

/** An unbounded async FIFO queue. */
export class Chan<T> implements AsyncIterable<T> {
  constructor();
  push(value: T): void;
  close(err?: unknown): void;
  next(): Promise<{ value: T | undefined; done: boolean }>;
  [Symbol.asyncIterator](): AsyncIterator<T>;
}

/** A pull-based async byte reader that buffers across chunk boundaries. */
export class AsyncReader {
  constructor(pull: () => Promise<Uint8Array | null | undefined>);
  remaining(): Uint8Array;
  consume(n: number): void;
  more(): Promise<boolean>;
  u8(): Promise<number>;
  take(n: number): Promise<Uint8Array>;
  varU(): Promise<bigint>;
  name(): Promise<string>;
}

// ----- value.js -------------------------------------------------------------
//
// The low-level value codec. `stream`/`future` values require the session's
// `sink` (collected by `invoke`/`accept`); without it they throw.

export function encodeValue(
  w: Writer,
  ty: Type,
  v: Value,
  sink?: { path: number[]; kind: "stream" | "future"; ty: Type; value: Value }[],
  path?: number[],
): void;
export function decodeValue(r: Reader, ty: Type, sink?: DecodeDeferred[], path?: number[]): Value;

// ----- mux.js ---------------------------------------------------------------

/** The wRPC framing protocol version byte. */
export const PROTOCOL: number;

/** A byte-stream transport: a duplex of `Uint8Array` chunks. */
export interface Transport {
  read(): Promise<Uint8Array | null | undefined>;
  write(bytes: Uint8Array): Promise<void> | void;
  closeWrite?(): Promise<void> | void;
}

export function fromWebStreams(duplex: {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
}): Transport;

/** A handle to one (sub-)stream of the connection's write side. */
export class Outgoing {
  constructor(mux: Mux, path: number[]);
  path: number[];
  write(data: Uint8Array): Promise<void>;
  index(rel: number[]): Outgoing;
}

/** The framing multiplexer over a {@link Transport}. */
export class Mux {
  constructor(transport: Transport);
  master: AsyncReader;
  writeHeader(instance: string, func: string): Promise<void>;
  start(): void;
  incoming(path: number[]): AsyncReader;
  outgoing(path: number[]): Outgoing;
  closeWrite(): Promise<void>;
}

// ----- session.js -----------------------------------------------------------

export function invoke(
  transport: Transport,
  instance: string,
  func: string,
  paramTypes: Type[],
  args: Value[],
  resultTypes?: Type[],
): Promise<{ results: Value[]; done: Promise<void> }>;

export interface Invocation {
  instance: string;
  func: string;
  mux: Mux;
  receiveParams(paramTypes: Type[]): Promise<{ params: Value[]; done: Promise<void> }>;
  sendResults(resultTypes: Type[], values: Value[]): Promise<void>;
}

export function accept(transport: Transport): Promise<Invocation>;
