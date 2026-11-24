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
  | { kind: "stream"; elem: Type | null }
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
 * `future` values are promises and `stream` values are `ReadableStream`s of
 * chunks, which are cancelled to stop reading them. The chunks of a `stream`
 * without an element type are their numbers of elements. Received data is not
 * buffered: a pending `stream` holds at most one chunk not read yet, and until
 * it is read, nothing else of its invocation is received, so the pending
 * values of an invocation must be consumed concurrently. If receiving them
 * fails, they stay pending.
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
  | ReadableStream<Chunk | number>
  | undefined;

/**
 * A component-model value to send: a decoded {@link Value}, or any other
 * representation the codec accepts, such as any iterable for a `list` or
 * `tuple`, a thunk for a `future`, an iterable or async iterable of chunks for
 * a `stream`, which is pulled only as fast as the transport takes its data,
 * and `null` for an `option`'s `none`. Scalars are converted like
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
  | (() => InputValue | Promise<InputValue>)
  | Promise<() => InputValue | Promise<InputValue>>
  | AsyncIterable<Iterable<InputValue> | number>
  | Iterable<Iterable<InputValue> | number>
  | null
  | undefined;

/** The thunk form of a `future` value to send, called once it is transmitted. */
export type FutureThunk<T> = () => T | Promise<T>;

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
  export function stream(elem?: Type | null): Type;
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
   * views of the chunks read, not copies. Fails with a {@link FrameDecodeError}.
   */
  next(): Promise<Uint8Array | null>;
  /**
   * The next invocation header, or `null` once the stream ends before one.
   * Names longer than `maxInstanceLength` and `maxNameLength` bytes, 1 KiB by
   * default, are rejected. Fails with a {@link HeaderDecodeError}.
   */
  header(options?: { maxInstanceLength?: number; maxNameLength?: number }): Promise<Header | null>;
}

/**
 * Frames written to and read from a byte stream pair, to invoke functions
 * over one after another or serve an invocation over. Their owner closes or
 * tears down the byte streams through {@link FrameWriter.writer} and
 * {@link FrameReader.reader}; once an invocation failed, it must tear them down.
 */
export class Framed implements Invoke, Serve<void> {
  constructor(
    writable: WritableStream<Uint8Array>,
    readable: ReadableStream<Uint8Array>,
    options?: { maxDepth?: number },
  );
  readonly tx: FrameWriter;
  readonly rx: FrameReader;
  /**
   * Invoke `instance#name` with `params` using `codec`: `params` are encoded
   * before anything is written, then the header and the parameters are sent.
   * Resolves to the results once they are received, along with the promise
   * of the rest of the invocation, which resolves once the pending parameters
   * are transmitted and the pending results received. The next invocation
   * starts once it resolved. Both fail with an {@link InvokeError}.
   */
  invoke<P, R>(
    codec: Codec<P, R>,
    instance: string,
    name: string,
    params: P,
  ): Promise<[R, Promise<void>]>;
  /**
   * Serve the invocation, whose header was read, using `codec` by calling `f`
   * with its parameters and transmitting the results it returns. Resolves once
   * all pending parameters are received and all pending results transmitted.
   * Nothing is transmitted if `f` fails. Fails with a {@link ServeError}.
   */
  serve<P, R>(codec: Codec<R, P>, f: (params: P) => R | Promise<R>): Promise<void>;
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
   * with the frames to serve it over, or `null` once the stream ends before one
   * or reading a header failed before. Fails with a {@link HeaderDecodeError}.
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
 * Encode `value` using `enc` into `w`, failing if any value nested in it is
 * pending, as `future` and `stream` values always are.
 */
export function pack<T>(enc: Encoder<T>, value: T, w: Writer): void;
/** Decode a value using `dec` from all of `r`, failing if any value nested in it is pending. */
export function unpack<T>(dec: Decoder<T>, r: Reader): T;

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
 * Receiving fails with a {@link ReceiveError} on a frame on a path, which is
 * not pending, on data after the end of a value, on `rx` ending before all
 * paths are closed, on invalid data, on an error of `rx` and on data for a
 * pending value whose consumer was cancelled. A consumer, which is not ready to take more data,
 * such as one of a stream whose chunks are not read, stalls receiving. Once
 * receiving failed, the pending values stay pending. `rx` must not be read
 * again before the receipt resolved.
 */
export function receive<T>(dec: Decoder<T>, rx: FrameReader): Promise<[T, Promise<void>]>;

/** The encoder of the parameters and the decoder of the results of an invocation, or vice versa. */
export interface Codec<E, D> {
  enc: Encoder<E>;
  dec: Decoder<D>;
}

/** A client invoking functions, such as {@link Framed} or {@link fromFetch}. */
export interface Invoke {
  /**
   * Invoke `instance#name` with `params` using `codec`, resolving to the
   * results once they are received, along with the promise of the rest of the
   * invocation, which resolves once the pending parameters are transmitted and
   * the pending results received. Both fail with an {@link InvokeError}.
   */
  invoke<P, R>(
    codec: Codec<P, R>,
    instance: string,
    name: string,
    params: P,
  ): Promise<[R, Promise<void>]>;
}

/**
 * A server serving an invocation, such as {@link Framed} or
 * {@link IncomingRequest}, resolving to `O`.
 */
export interface Serve<O> {
  /**
   * Serve the invocation using `codec` by calling `f` with its parameters,
   * failing with a {@link ServeError}.
   */
  serve<P, R>(codec: Codec<R, P>, f: (params: P) => R | Promise<R>): Promise<O>;
}

// ----- rpc.js ---------------------------------------------------------------

/**
 * Invoke `instance#name` with `values` of `params` using `clt`, resolving to
 * the values of `results` once they are received, along with the promise of
 * the rest of the invocation.
 */
export function invoke(
  clt: Invoke,
  instance: string,
  name: string,
  params: Type[],
  values: InputValue[],
  results: Type[],
): Promise<[Value[], Promise<void>]>;

/**
 * Serve an invocation using `srv` by calling `f` with its values of `params`
 * and transmitting the values of `results` it returns.
 */
export function serve<O>(
  srv: Serve<O>,
  params: Type[],
  results: Type[],
  f: (...params: any[]) => InputValue[] | Promise<InputValue[]>,
): Promise<O>;

// ----- error.js -------------------------------------------------------------

/**
 * The error of decoding an invocation header: an unsupported protocol
 * `version` (`unsupportedVersion`), a name `len` bytes long, longer than
 * `max` (`nameTooLong`), a name, which is not UTF-8 (`invalidName`), a name
 * length, which does not fit in a `u32` (`overflow`), the byte stream ending
 * inside a header (`truncated`), or reading the byte stream failing (`io`).
 */
export class HeaderDecodeError extends Error {
  readonly kind:
    "unsupportedVersion" | "nameTooLong" | "invalidName" | "overflow" | "truncated" | "io";
  readonly version?: number;
  readonly len?: number;
  readonly max?: number;
}

/**
 * The error of decoding a frame: a path `depth` elements long, longer than
 * `max` (`pathTooDeep`), a path or length, which does not fit in a `u32`
 * (`overflow`), the byte stream ending inside a frame (`truncated`), or
 * reading the byte stream failing (`io`).
 */
export class FrameDecodeError extends Error {
  readonly kind: "pathTooDeep" | "overflow" | "truncated" | "io";
  readonly depth?: number;
  readonly max?: number;
}

/**
 * The error of receiving a value: data for a pending value whose consumer was
 * cancelled (`unread`), a frame on a path, which is not pending
 * (`unknownPath`), data after the end of a value or after its path was closed
 * (`dataAfterEnd`), the frames ending or a path closed before its value was
 * complete (`truncated`), a value decoder rejecting the data (`decode`), or
 * reading the frames failing (`frame`, caused by a {@link FrameDecodeError}).
 */
export class ReceiveError extends Error {
  constructor(
    kind: "unread" | "unknownPath" | "dataAfterEnd" | "truncated" | "decode" | "frame",
    cause?: unknown,
  );
  readonly kind: "unread" | "unknownPath" | "dataAfterEnd" | "truncated" | "decode" | "frame";
}

/**
 * The error of serving an invocation, by the side at fault: receiving the
 * parameters or the rest of the invocation failed (`request`, caused by a
 * {@link ReceiveError}), the handler cancelled a pending parameter, for which
 * data was received (`unread`), the handler failed (`handler`), or encoding or
 * transmitting the results failed (`response`).
 */
export class ServeError extends Error {
  readonly kind: "request" | "unread" | "handler" | "response";
}

/**
 * The error of invoking a function, by the side at fault: encoding or
 * transmitting the parameters or obtaining the response, such as `fetch`
 * rejecting, failed (`request`), the client cancelled a pending result, for
 * which data was received (`unread`), or receiving the results or the rest of
 * the invocation failed (`response`, caused by a {@link ReceiveError}).
 */
export class InvokeError extends Error {
  readonly kind: "request" | "unread" | "response";
}

// ----- http.js --------------------------------------------------------------

/** The media type of wRPC request and response bodies. */
export const MEDIA_TYPE: "application/vnd.bytecodealliance.wrpc";

/** Options of {@link fromFetch}: the `fetch` request options, except for the method and body. */
export interface FetchOptions extends Omit<RequestInit, "method" | "body"> {
  /** The `fetch` implementation to use, `globalThis.fetch` by default. */
  fetch?: typeof fetch;
  /** Buffer the request body and send it once the parameters are transmitted. */
  buffer?: boolean;
}

/**
 * A client invoking each function with a `POST` request to `input`, with the
 * {@link MEDIA_TYPE} unless `headers` set one, and reading the body of the
 * response, whatever its status. To act on the response, wrap `fetch`,
 * failing the invocation by rejecting, with an {@link InvokeError} of kind
 * `request` caused by the rejection. Once an invocation failed, its request
 * and response bodies are torn down.
 */
export function fromFetch(input: RequestInfo | URL, options?: FetchOptions): Invoke;

/**
 * Receive the invocation header of `request`, resolving to it along with the
 * request to serve the invocation over. Fails with a {@link HeaderDecodeError}.
 */
export function accept(
  request: Request,
  options?: { maxDepth?: number; maxInstanceLength?: number; maxNameLength?: number },
): Promise<[Header, IncomingRequest]>;

/** A request carrying an invocation. */
export class IncomingRequest implements Serve<Response> {
  /**
   * Serve the invocation using `codec` by calling `f` with its parameters,
   * resolving to the response, with the {@link MEDIA_TYPE}, once the results
   * are encoded. Its body transmits the pending results, while the rest of
   * the request is received; a failure after that tears both down.
   */
  serve<P, R>(codec: Codec<R, P>, f: (params: P) => R | Promise<R>): Promise<Response>;
}
