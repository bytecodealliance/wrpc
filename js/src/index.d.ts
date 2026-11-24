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
  | { kind: "list"; elem: Type; length?: number }
  | { kind: "map"; key: Type; value: Type }
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
 * `stream` values are {@link Chan}s of chunks, or `ReadableStream`s of chunks
 * if decoded by a {@link Decoder}, which can be cancelled.
 */
export type Value =
  | boolean
  | number
  | bigint
  | string
  | TypedArray
  | Value[]
  | Map<Value, Value>
  | { [field: string]: Value }
  | { kind: string; value?: Value }
  | Promise<Value>
  | Chan<Chunk>
  | ReadableStream<Chunk>
  | undefined;

/**
 * A component-model value to send: a decoded {@link Value}, or any other
 * representation the codec accepts, such as any iterable for a `list` or
 * `tuple`, a thunk for a `future`, an iterable or async iterable of chunks for
 * a `stream`, and `null` for an `option`'s `none`. Scalars are converted like
 * the component model's JS-API draft does, except that integers must be
 * `number`s (`bigint`s for `s64` and `u64`) in range.
 */
export type InputValue =
  | boolean
  | number
  | bigint
  | string
  | TypedArray
  | Iterable<InputValue>
  | Map<InputValue, InputValue>
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
  export function list(elem: Type, length?: number): Type;
  export function map(key: Type, value: Type): Type;
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
  name(max?: number): string;
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
  name(max?: number): Promise<string>;
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

// ----- frame.js -------------------------------------------------------------

/** The wRPC framing protocol version byte. */
export const PROTOCOL: number;

/** An invocation header. */
export interface Header {
  instance: string;
  name: string;
}

/** Writes invocation headers and frames to a byte stream. */
export class FrameWriter {
  constructor(writable: WritableStream<Uint8Array>);
  /** The writer of the byte stream, through which its owner closes or aborts it. */
  readonly writer: WritableStreamDefaultWriter<Uint8Array>;
  header(header: Header): Promise<void>;
  /** Write `data` as frames on `path`, writing nothing if it is empty. */
  write(path: readonly number[], data: Uint8Array): Promise<void>;
  /** Write the frame closing `path`. */
  close(path: readonly number[]): Promise<void>;
}

/** Reads invocation headers and frames from a byte stream. */
export class FrameReader {
  /** Paths longer than `maxDepth` elements, 64 by default, are rejected. */
  constructor(readable: ReadableStream<Uint8Array>, options?: { maxDepth?: number });
  /** The reader of the byte stream, through which its owner cancels it. */
  readonly reader: ReadableStreamDefaultReader<Uint8Array>;
  /** The path of the data last returned by {@link next}. */
  readonly path: readonly number[];
  /**
   * The next chunk of the data of a frame on {@link path}, empty for the frame
   * closing it, or `null` once the stream ends between frames. Chunks are
   * views of the chunks read, not copies.
   */
  next(): Promise<Uint8Array | null>;
  /**
   * The next invocation header, or `null` once the stream ends before one.
   * Names longer than `maxInstanceLength` and `maxNameLength` bytes, 1 KiB by
   * default, are rejected.
   */
  header(options?: { maxInstanceLength?: number; maxNameLength?: number }): Promise<Header | null>;
}

/** Frames written to and read from a byte stream pair. */
export class Framed {
  constructor(
    writable: WritableStream<Uint8Array>,
    readable: ReadableStream<Uint8Array>,
    options?: { maxDepth?: number },
  );
  readonly tx: FrameWriter;
  readonly rx: FrameReader;
}

/** Accepts the invocations sent over a byte stream pair one after another. */
export class Listener {
  constructor(
    writable: WritableStream<Uint8Array>,
    readable: ReadableStream<Uint8Array>,
    options?: { maxDepth?: number; maxInstanceLength?: number; maxNameLength?: number },
  );
  /** The frames of the byte stream pair, through which its owner closes or tears it down. */
  readonly io: Framed;
  /**
   * The header of the next invocation, once the previous one was served, along
   * with the frames to serve it over, or `null` once the stream ends before one.
   * Once serving an invocation failed, the byte streams must be torn down.
   */
  accept(): Promise<[Header, Framed] | null>;
}

// ----- codec.js -------------------------------------------------------------

/** An encoder of values, which transmits the pending values nested in the last encoded value. */
export interface Encoder<T = InputValue> {
  /** Encode `value` into `w`. */
  encode(w: Writer, value: T): void;
  /**
   * Write the frames of the pending values nested in the last encoded value,
   * whose paths are nested in `path`, resolving once all of them are closed.
   */
  transmit(path: readonly number[], tx: FrameWriter): Promise<void>;
  /** Whether nothing in the last encoded value is pending. */
  readonly complete: boolean;
}

/** A decoder of values, which receives the pending values nested in the last decoded value. */
export interface Decoder<T = Value> {
  /**
   * Decode a value from `r` as a resumable decoder: when `r` runs out, it
   * yields the number of bytes it needs from `r.pos` on, and continues once
   * `r` holds them.
   */
  decode(r: Reader): Generator<number, T, void>;
  /**
   * Receive `data`, the data of a frame on the elements of `path` from `at` on,
   * relative to the last decoded value, which closes that path if empty,
   * resolving once `data` is taken.
   */
  receive(path: readonly number[], at: number, data: Uint8Array): Promise<void> | void;
  /** Whether nothing in the last decoded value is pending. */
  readonly complete: boolean;
}

/** The encoder of the values of `ty`. */
export function encoder(ty: Type): Encoder;
/** The decoder of the values of `ty`. */
export function decoder(ty: Type): Decoder;

/**
 * Send `value`, the parameters when invoking and the results when serving, on
 * the root path using `enc`, then the pending values nested in it. `value` is
 * encoded before anything is written. Resolves once all paths are closed.
 */
export function transmit<T>(enc: Encoder<T>, tx: FrameWriter, value: T): Promise<void>;

/**
 * Receive a value, the results when invoking and the parameters when serving,
 * from the root path using `dec`, returning it and the promise receiving the
 * pending values nested in it, which resolves once all paths are closed.
 * Receiving fails on a frame on a path, which is not pending, on data after
 * the end of a value, on `rx` ending before all paths are closed, on an error
 * of `rx` and, with {@link UnreadError}, on data for a pending value whose
 * consumer was cancelled. A consumer, which is not ready to take more data,
 * such as one of a stream whose chunks are not read, stalls receiving. Once
 * receiving failed, the pending values stay pending. `rx` must not be read
 * again before the receipt resolved.
 */
export function receive<T>(dec: Decoder<T>, rx: FrameReader): Promise<[T, Promise<void>]>;

/** The error of receiving data for a pending value whose consumer was cancelled. */
export class UnreadError extends Error {}

/** The error for a frame received on `path`, which is not pending. */
export function errorUnknownPath(path: readonly number[]): Error;
/** The error for data received after the end of a value. */
export function errorDataAfterEnd(): Error;

// ----- mux.js ---------------------------------------------------------------

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
 * path that may still be declared is buffered until it is; any frame for any
 * other path, or for a closed one, fails the invocation.
 */
export class Mux {
  /**
   * Aborts `transport` and throws a `TypeError` if it does not implement
   * `closeWrite`. `master` is the reader over `transport`, which must be
   * shared by the invocations on it, since it holds the bytes read past the
   * previous one. Without it, `transport` carries this invocation alone, and
   * data following it fails the invocation.
   */
  constructor(transport: Transport, master?: AsyncReader);
  master: AsyncReader;
  /**
   * Resolves once the root and every sub-stream expected are closed and no
   * more may be expected, or the peer ended transmit and every frame it sent
   * was routed, and the write side was half-closed, unless the multiplexer is
   * aborted first: then it rejects with the error it was aborted with.
   * Without a `master`, it also waits for the peer to end transmit.
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
   * root `indices`, at paths at most `depth` long, so that any frame for any
   * other path fails the invocation as soon as it arrives.
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

// ----- http.js --------------------------------------------------------------

/** The media type of wRPC request and response bodies. */
export const MEDIA_TYPE: "application/vnd.bytecodealliance.wrpc";

/** Options of {@link fromFetch}: the `fetch` request options, except for the method, body and signal. */
export interface FetchOptions extends Omit<RequestInit, "method" | "body" | "signal"> {
  /** The `fetch` implementation to use, `globalThis.fetch` by default. */
  fetch?: typeof fetch;
  /** Buffer the request body and send it once the client ends transmit. */
  buffer?: boolean;
}

/**
 * A client transport for one invocation, to {@link invoke} over, sending a
 * `POST` request to `input`, with the {@link MEDIA_TYPE} unless `headers` set
 * one, and reading the body of the response, whatever its status. To act on
 * the response, wrap `fetch`.
 */
export function fromFetch(input: RequestInfo | URL, options?: FetchOptions): Transport;

/**
 * A server transport for one invocation over `request`, to {@link accept}, with
 * the response carrying the server's data, with the {@link MEDIA_TYPE}.
 */
export function fromRequest(request: Request): { transport: Transport; response: Response };

// ----- session.js -----------------------------------------------------------

/**
 * `done` resolves once all asynchronous parameters and results have been
 * transmitted; `finished` once the peer has also closed every path it sent and
 * all the data it sent has been decoded, rejecting with any failure of the
 * invocation, including those after `done`. The transport carries this
 * invocation alone: `finished` also waits for the peer to end transmit, and
 * data following the invocation fails it.
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
   * Resolves once the parameters have been received, the peer has closed
   * every path it sent, the results have been sent and all the data the peer
   * sent has been decoded, rejecting with any failure of the invocation,
   * including those after the parameters' `done`.
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

/**
 * Accept an invocation. The transport carries this invocation alone:
 * `finished` also waits for the peer to end transmit, and data following the
 * invocation fails it. Instance and function names longer than
 * `maxInstanceLength` and `maxNameLength` bytes, 1 KiB by default, are
 * rejected.
 */
export function accept(
  transport: Transport,
  options?: { maxInstanceLength?: number; maxNameLength?: number },
): Promise<Invocation>;

/**
 * Serve the invocations on `transport` one after another, until the peer ends
 * transmit in place of the next one, and then half-close the write side,
 * passing each accepted invocation to `f`, which must start receiving its
 * parameters before it resolves, and reading the next one once it is finished.
 */
export function serve(
  transport: Transport,
  f: (inv: Invocation) => Promise<void>,
  options?: { maxInstanceLength?: number; maxNameLength?: number },
): Promise<void>;
