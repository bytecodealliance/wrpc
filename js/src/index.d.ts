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
 * value, naming the relative sub-stream `path` and the placeholder to feed, or
 * for a `stream` sent `inline` as a single ready chunk whose elements carry
 * pending values.
 */
export type DecodeDeferred =
  | { path: number[]; kind: "future"; ty: Type; resolve: (value: Value) => void; reject: (err: unknown) => void }
  | { path: number[]; kind: "stream"; ty: Type; queue: Chan<Value[] | Uint8Array> }
  | { path: number[]; kind: "inline"; queue: Chan<Value[] | Uint8Array> };

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
  /** The buffer length the last end-of-input underflow needed. */
  need: number;
  u8(): number;
  peek(): number;
  take(n: number): Uint8Array;
  varU(bits?: number): bigint;
  varS(bits?: number): bigint;
  name(): string;
}

/**
 * Run the synchronous read `f` as one step of a resumable decoder: if the
 * input runs out partway, rewind `r` and yield the number of bytes the step
 * needs from there, then retry it once they are buffered.
 */
export function atomic<T>(r: Reader, f: (r: Reader) => T): Generator<number, T, void>;

/**
 * An async FIFO queue. Leaving a `for await` loop over it early cancels it.
 */
export class Chan<T> implements AsyncIterable<T> {
  constructor(capacity?: number);
  readonly capacity: number;
  readonly closed: boolean;
  readonly cancelled: boolean;
  /** The number of values handed to the consumer. */
  readonly taken: number;
  /**
   * Call `f` once the consumer cancels the channel, or now if it already has.
   * Hooks are dropped once the consumer has received the end of the channel.
   */
  onCancel(f: () => void): void;
  push(value: T): void;
  /** Push `value` once fewer than `capacity` values are buffered; `false` if closed instead. */
  send(value: T): Promise<boolean>;
  close(err?: unknown): void;
  /** Stop consuming: drop the buffered values, close (failed with `err`, if given and not already failed), and call the `onCancel` hooks. */
  cancel(err?: unknown): void;
  next(): Promise<{ value: T | undefined; done: boolean }>;
  [Symbol.asyncIterator](): AsyncIterator<T>;
}

/** A pull-based async byte reader that buffers across chunk boundaries. */
export class AsyncReader {
  constructor(pull: () => Promise<Uint8Array | null | undefined>);
  remaining(): Uint8Array;
  consume(n: number): void;
  fill(n: number): Promise<boolean>;
  /**
   * Run the resumable decoder `decode` over the buffered bytes: each time it
   * yields the number of bytes it needs next, pull more input.
   */
  decode<T>(decode: (r: Reader) => Generator<number, T, void>): Promise<T>;
  u8(): Promise<number>;
  take(n: number): Promise<Uint8Array>;
  varU(bits?: number): Promise<bigint>;
  name(): Promise<string>;
}

// ----- value.js -------------------------------------------------------------
//
// The low-level value codec. Pending `stream`/`future` values require the
// session's `sink` (collected by `invoke`/`accept`); without it they throw.

export function encodeValue(
  w: Writer,
  ty: Type,
  v: Value,
  sink?: { path: number[]; kind: "stream" | "future"; ty: Type; value: Value }[],
  path?: number[],
): void;
export function decodeValue(r: Reader, ty: Type, sink?: DecodeDeferred[], path?: number[]): Value;
/**
 * {@link decodeValue} as a resumable decoder: when `r` runs out, it yields the
 * number of bytes it needs from `r.pos` on, and continues once `r` holds them.
 */
export function decodeSteps(
  r: Reader,
  ty: Type,
  sink?: DecodeDeferred[],
  path?: number[],
): Generator<number, Value, void>;

// ----- mux.js ---------------------------------------------------------------

/** The wRPC framing protocol version byte. */
export const PROTOCOL: number;

/** The error a discarded sub-stream's reader fails with. */
export const DISCARDED: Error;

/** A byte-stream transport: a duplex of `Uint8Array` chunks. */
export interface Transport {
  read(): Promise<Uint8Array | null | undefined>;
  write(bytes: Uint8Array): Promise<void> | void;
  closeWrite?(): Promise<void> | void;
  /** Tear down both directions after a failure. */
  abort?(reason: unknown): Promise<void> | void;
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

/**
 * The framing multiplexer over a {@link Transport}. Inbound data is accepted
 * only for the root sub-stream `[]` and the sub-streams declared with `seal`.
 */
export class Mux {
  constructor(transport: Transport);
  master: AsyncReader;
  writeHeader(instance: string, func: string): Promise<void>;
  start(): void;
  /** The reader for the expected sub-stream at `path`. */
  incoming(path: number[]): AsyncReader;
  /**
   * Declare the sub-streams of the pending values in the value decoded at
   * `path` (the root tuple, a future's value, or a stream's element, in
   * order), with paths relative to it.
   */
  seal(path: number[], expected: { path: number[]; nested: boolean }[]): void;
  /** Declare that the sub-stream at `path` has no more data: its stream ended, or its future's value was decoded. */
  finish(path: number[]): void;
  /** Discard the sub-stream at `path` and those nested in its children from index `from` (default 0) on. */
  discard(path: number[], from?: number): void;
  /** Call `f` once the sub-stream at `path` is discarded. */
  ondiscard(path: number[], f: () => void): void;
  outgoing(path: number[]): Outgoing;
  abort(err?: unknown): void;
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
