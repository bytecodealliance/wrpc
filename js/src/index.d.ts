// Type definitions for the `wrpc` package.

/** A structural wRPC/WIT type. */
export type Type =
  | {
      kind:
        | "bool"
        | "s8"
        | "u8"
        | "s16"
        | "u16"
        | "s32"
        | "u32"
        | "s64"
        | "u64"
        | "f32"
        | "f64"
        | "char"
        | "string";
    }
  | { kind: "list"; elem: Type }
  | { kind: "option"; some: Type }
  | { kind: "result"; ok: Type | null; err: Type | null }
  | { kind: "tuple"; elems: Type[] }
  | { kind: "record"; fields: { name: string; key: string; ty: Type }[] }
  | { kind: "variant"; cases: { name: string; ty: Type | null }[] }
  | { kind: "enum"; names: string[] }
  | { kind: "flags"; flags: { name: string; key: string }[] }
  | { kind: "future"; some: Type | null }
  | { kind: "stream"; elem: Type }
  | { kind: "own" | "borrow"; name: string };

/** The typed arrays that lists of the numeric types map to. */
export type TypedArray =
  | Int8Array
  | Uint8Array
  | Int16Array
  | Uint16Array
  | Int32Array
  | Uint32Array
  | BigInt64Array
  | BigUint64Array
  | Float32Array
  | Float64Array;

/** A `list` value or `stream` chunk: a typed array for the numeric types. */
export type Chunk = Value[] | TypedArray;

/**
 * The JavaScript representation of a decoded component-model value. Decoded
 * `stream` values are {@link Chan}s of chunks, which can be cancelled.
 */
export type Value =
  | boolean
  | number
  | bigint
  | string
  | TypedArray
  | Value[]
  | { [field: string]: Value }
  | { kind: string; value?: Value }
  | Promise<Value>
  | Chan<Chunk>
  | undefined;

/**
 * A component-model value to send: a decoded {@link Value}, or any other
 * representation the codec accepts, such as any iterable for a `list` or
 * `tuple`, a thunk for a `future`, an iterable or async iterable of chunks for
 * a `stream`, and `null` for an `option`'s `none`. Scalars are converted like
 * the component model's JS-API draft does, e.g. integers wrap.
 */
export type InputValue =
  | boolean
  | number
  | bigint
  | string
  | TypedArray
  | Iterable<InputValue>
  | { [field: string]: InputValue }
  | { kind: string; value?: InputValue }
  | Promise<InputValue>
  | (() => InputValue)
  | AsyncIterable<Iterable<InputValue>>
  | null
  | undefined;

/**
 * A descriptor recorded by {@link decodeValue} for a pending `stream`/`future`
 * value, naming the relative sub-stream `path` and the placeholder to feed, or
 * for a `stream` sent `inline` as a single ready chunk whose elements carry
 * pending values.
 */
export type DecodeDeferred =
  | {
      path: number[];
      kind: "future";
      ty: Type | null;
      resolve: (value: Value) => void;
    }
  | { path: number[]; kind: "stream"; ty: Type; queue: Chan<Chunk> }
  | { path: number[]; kind: "inline"; queue: Chan<Chunk> };

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
  export function future(some?: Type | null): Type;
  export function stream(elem: Type): Type;
  export function result(ok?: Type | null, err?: Type | null): Type;
  export function tuple(...elems: Type[]): Type;
  export function record(
    fields: Record<string, Type> | { name: string; ty: Type }[],
  ): Extract<Type, { kind: "record" }>;
  export function variant(
    cases: Record<string, Type | null> | { name: string; ty: Type | null }[],
  ): Type;
  export function enumType(names: string[]): Type;
  export { enumType as enum };
  export function flags(names: string[]): Extract<Type, { kind: "flags" }>;
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
  /** The error the channel was failed with, if any. */
  readonly error: unknown;
  /**
   * Call `f` with the buffered values dropped once the consumer cancels the
   * channel, or now, with none, if it already has. Hooks are dropped once the
   * consumer has received the end of the channel.
   */
  onCancel(f: (dropped: T[]) => void): void;
  push(value: T): void;
  /** Push `value` once fewer than `capacity` values are buffered; `false` if closed instead. */
  send(value: T): Promise<boolean>;
  close(err?: unknown): void;
  /** Stop consuming: drop the buffered values, close (failed with `err`, if given and not already failed), and call the `onCancel` hooks with the values dropped. */
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
  v: InputValue,
  sink?: { path: number[]; kind: "stream" | "future"; ty: Type | null; value: InputValue }[],
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
  /** Half-close the write side, communicating the end of transmit to the peer. */
  closeWrite(): Promise<void> | void;
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
  close(): Promise<void>;
  index(rel: number[]): Outgoing;
}

/**
 * The framing multiplexer over a {@link Transport}. Inbound data is only
 * accepted for the sub-streams the decoded values expect: the root sub-stream
 * `[]`, and those declared with `seal` as each value carrying pending
 * `stream`/`future` values is decoded, until they are finished. Data for a
 * path that may still be declared is buffered until it is; data for any other
 * path, or for a closed one, fails the invocation, while an empty frame for it
 * is discarded.
 */
export class Mux {
  /**
   * Aborts `transport` and throws a `TypeError` if it does not implement
   * `closeWrite`.
   */
  constructor(transport: Transport);
  master: AsyncReader;
  /**
   * Resolves once the peer ended transmit and every frame it sent was routed,
   * and the write side was half-closed, unless the multiplexer is aborted
   * first: then it rejects with the error it was aborted with.
   */
  readonly ended: Promise<void>;
  /** Rejects with the error the multiplexer is aborted with, once it is. */
  readonly failed: Promise<never>;
  writeHeader(instance: string, func: string): Promise<void>;
  start(): void;
  /** The reader for the expected sub-stream at `path`. */
  incoming(path: number[]): AsyncReader;
  /**
   * Declare that sub-streams nested in the root may only be expected at the
   * root `indices`, at paths at most `depth` long, so that data for any other
   * path fails the invocation as soon as it arrives, while an empty frame for
   * it is discarded.
   */
  restrict(indices: Iterable<number>, depth: number): void;
  /**
   * Declare the sub-streams of the pending values in the value decoded at
   * `path` (the root tuple, a future's value, or a stream's element, in
   * order), with paths relative to it. Returns whether they are declared:
   * nothing is if the value was forgotten, as discarded ones are.
   */
  seal(path: number[], expected: { path: number[]; nested: boolean }[]): boolean;
  /**
   * Declare that the sub-stream at `path` has no more data: its stream ended,
   * or its future's value was decoded. Data of it left unread in `reader`
   * fails the invocation.
   */
  finish(path: number[], reader?: AsyncReader): void;
  /**
   * Discard the sub-streams nested in elements `from` up to `to` of the
   * stream at `path`.
   */
  discard(path: number[], from: number, to: number): void;
  /** Call `f` once the sub-stream expected at `path` is discarded, or now if it already is. */
  onDiscard(path: number[], f: () => void): void;
  outgoing(path: number[]): Outgoing;
  abort(err?: unknown): void;
  /** Flush pending writes and half-close the write side. */
  closeWrite(): Promise<void>;
}

// ----- session.js -----------------------------------------------------------

/**
 * `done` resolves once all asynchronous parameters and results have been
 * transmitted; `finished` once the peer has also ended transmit and all the
 * data it sent has been decoded, rejecting with any failure of the
 * invocation, including those after `done`.
 */
export function invoke(
  transport: Transport,
  instance: string,
  func: string,
  paramTypes: Type[],
  args: InputValue[],
  resultTypes?: Type[],
): Promise<{ results: Value[]; done: Promise<void>; finished: Promise<void> }>;

export interface Invocation {
  instance: string;
  func: string;
  mux: Mux;
  /**
   * Resolves once the parameters have been received, the peer has ended
   * transmit, the results have been sent and all the data the peer sent has
   * been decoded, rejecting with any failure of the invocation, including
   * those after the parameters' `done`.
   */
  finished: Promise<void>;
  /**
   * Start reading the frames of the invocation and decode the parameters
   * against `paramTypes`. It must be called, with `[]` for a function without
   * parameters, for `finished` to resolve.
   */
  receiveParams(paramTypes: Type[]): Promise<{ params: Value[]; done: Promise<void> }>;
  sendResults(resultTypes: Type[], values: InputValue[]): Promise<void>;
}

export function accept(transport: Transport): Promise<Invocation>;
