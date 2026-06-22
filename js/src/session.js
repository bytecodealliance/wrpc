// The asynchronous session layer: invoking a function and serving one over a
// framed transport, with full support for `stream` and `future` parameters and
// results.
//
// An invocation's synchronous values travel in the root frame (path `[]`);
// each pending `stream`/`future` is transmitted concurrently over the framed
// sub-stream addressed by its structural index path (see `SPEC.md`). The value
// codec (`value.js`) writes a pending marker and records a descriptor for every
// such value; the helpers below walk those descriptors, spawning a reader or
// writer task per sub-stream.

import { Reader, Writer, isUnexpectedEof } from "./io.js";
import { Mux, PROTOCOL } from "./mux.js";
import { decodeValue, encodeValue } from "./value.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./mux.js").Transport} Transport */
/** @typedef {import("./mux.js").Outgoing} Outgoing */

// ----- encoding side --------------------------------------------------------

/** Resolve a `future` value: a plain value, a thunk, or a promise.
 * @param {any} v */
async function resolveFuture(v) {
  if (typeof v === "function") v = v();
  return await v;
}

/**
 * Yield the chunks of a `stream` source. A `stream<u8>` may be given as a
 * single `Uint8Array`; otherwise the source is any (async-)iterable of chunks
 * (arrays of elements, or `Uint8Array`s for `stream<u8>`).
 * @param {any} src
 * @param {Type} elem
 */
async function* streamChunks(src, elem) {
  if (src === null || src === undefined) return;
  if (elem.kind === "u8" && src instanceof Uint8Array) {
    if (src.length) yield src;
    return;
  }
  if (typeof src[Symbol.asyncIterator] === "function") {
    for await (const chunk of src) yield chunk;
    return;
  }
  if (typeof src[Symbol.iterator] === "function") {
    for (const chunk of src) yield chunk;
    return;
  }
  throw new TypeError("expected a `stream` source: an iterable or async-iterable of chunks");
}

/** @param {any} chunk @param {Type} elem @returns {any[]} */
function chunkItems(chunk, elem) {
  if (elem.kind === "u8") return Array.from(chunk);
  if (!Array.isArray(chunk)) throw new TypeError("expected an array for a `stream` chunk");
  return chunk;
}

/**
 * Transmit one deferred value descriptor over `out`.
 * @param {Outgoing} out
 * @param {{ kind: "stream" | "future", ty: Type, value: any }} entry
 */
function writeDeferred(out, entry) {
  return entry.kind === "future"
    ? writeFuture(out, entry.ty, entry.value)
    : writeStream(out, entry.ty, entry.value);
}

/** @param {Outgoing} out @param {Type} ty @param {any} value */
async function writeFuture(out, ty, value) {
  const v = await resolveFuture(value);
  const w = new Writer();
  /** @type {any[]} */
  const sink = [];
  encodeValue(w, ty, v, sink, []);
  await out.write(w.finish());
  await Promise.all(sink.map((e) => writeDeferred(out.index(e.path), e)));
}

/** @param {Outgoing} out @param {Type} elem @param {any} src */
async function writeStream(out, elem, src) {
  let base = 0;
  /** @type {Promise<void>[]} */
  const tasks = [];
  for await (const chunk of streamChunks(src, elem)) {
    const w = new Writer();
    if (elem.kind === "u8") {
      const bytes = chunk instanceof Uint8Array ? chunk : Uint8Array.from(chunk);
      // An empty chunk is indistinguishable from the stream terminator.
      if (bytes.length === 0) continue;
      w.varU(bytes.length);
      w.bytes(bytes);
      base += bytes.length;
    } else {
      const items = chunkItems(chunk, elem);
      if (items.length === 0) continue;
      w.varU(items.length);
      items.forEach((item, k) => {
        /** @type {any[]} */
        const sink = [];
        encodeValue(w, elem, item, sink, []);
        for (const e of sink) tasks.push(writeDeferred(out.index([base + k, ...e.path]), e));
      });
      base += items.length;
    }
    await out.write(w.finish());
  }
  const end = new Writer();
  end.varU(0); // a stream is terminated by an empty chunk
  await out.write(end.finish());
  await Promise.all(tasks);
}

// ----- decoding side --------------------------------------------------------

/**
 * Decode one value of type `ty` from the async reader `ar`, pulling more input
 * until a whole value is available. Returns the value and the descriptors of
 * any pending `stream`/`future` values nested within it (with paths relative to
 * the value).
 * @param {import("./io.js").AsyncReader} ar
 * @param {Type} ty
 */
async function decodeOne(ar, ty) {
  for (;;) {
    const r = new Reader(ar.remaining());
    /** @type {any[]} */
    const sink = [];
    try {
      const value = decodeValue(r, ty, sink, []);
      ar.consume(r.pos);
      return { value, sink };
    } catch (err) {
      if (isUnexpectedEof(err) && (await ar.more())) continue;
      if (isUnexpectedEof(err)) {
        throw new RangeError("unexpected end of input while decoding a value");
      }
      throw err;
    }
  }
}

/**
 * Decode a tuple of `types` from the root channel reader, returning the decoded
 * values and the absolute-path descriptors of pending sub-streams.
 * @param {import("./io.js").AsyncReader} root
 * @param {Type[]} types
 */
async function decodeTuple(root, types) {
  const values = [];
  /** @type {any[]} */
  const sink = [];
  for (let i = 0; i < types.length; i++) {
    const { value, sink: nested } = await decodeOne(root, types[i]);
    values.push(value);
    for (const e of nested) sink.push({ ...e, path: [i, ...e.path] });
  }
  return { values, sink };
}

/**
 * Receive one deferred value over `ar` and feed its placeholder.
 * @param {any} ar
 * @param {import("./index.d.ts").DecodeDeferred} entry
 */
async function readDeferred(ar, entry) {
  if (entry.kind === "future") {
    const { value, sink } = await decodeOne(ar, entry.ty);
    entry.resolve(value);
    await Promise.all(sink.map((e) => readDeferred(e.path.length ? ar.index(e.path) : ar, e)));
    return;
  }
  await readStream(ar, entry.ty, entry.queue);
}

/** @param {any} ar @param {Type} elem @param {any} queue */
async function readStream(ar, elem, queue) {
  let base = 0;
  /** @type {Promise<void>[]} */
  const tasks = [];
  try {
    for (;;) {
      let len;
      try {
        len = Number(await ar.varU());
      } catch (err) {
        // The peer closed the connection without an explicit empty terminator.
        if (isUnexpectedEof(err)) break;
        throw err;
      }
      if (len === 0) break;
      if (elem.kind === "u8") {
        queue.push(await ar.take(len));
      } else {
        const chunk = new Array(len);
        for (let k = 0; k < len; k++) {
          const { value, sink } = await decodeOne(ar, elem);
          chunk[k] = value;
          for (const e of sink) tasks.push(readDeferred(ar.index([base + k, ...e.path]), e));
        }
        queue.push(chunk);
      }
      base += len;
    }
  } catch (err) {
    // Surface a transport/decode failure to whoever is consuming the stream,
    // rather than leaving the consumer hanging on a queue that never closes.
    queue.close(err);
    throw err;
  }
  queue.close();
  await Promise.all(tasks);
}

/**
 * Run every pending sub-stream reader for a decoded tuple. Each descriptor's
 * path is absolute (rooted at the root channel).
 * @param {any} root
 * @param {any[]} sink
 */
function runReadSink(root, sink) {
  return Promise.all(sink.map((e) => readDeferred(root.index(e.path), e)));
}

// ----- public API -----------------------------------------------------------

/**
 * Invoke `func` on `instance` over `transport`, sending `args` (encoded against
 * `paramTypes`) and decoding the reply against `resultTypes`.
 *
 * Returns the decoded `results` immediately once the synchronous reply frame
 * arrives — `stream` results are async iterables of chunks and `future`
 * results are promises, both fed as their data arrives — together with a
 * `done` promise that resolves once all asynchronous I/O (parameters and
 * results) has completed.
 *
 * @param {Transport} transport
 * @param {string} instance
 * @param {string} func
 * @param {Type[]} paramTypes
 * @param {any[]} args
 * @param {Type[]} [resultTypes]
 * @returns {Promise<{ results: any[], done: Promise<void> }>}
 */
export async function invoke(transport, instance, func, paramTypes, args, resultTypes = []) {
  if (args.length !== paramTypes.length) {
    throw new RangeError(`expected ${paramTypes.length} argument(s), got ${args.length}`);
  }
  const mux = new Mux(transport);
  await mux.writeHeader(instance, func);
  mux.start();

  const w = new Writer();
  /** @type {any[]} */
  const psink = [];
  paramTypes.forEach((ty, i) => encodeValue(w, ty, args[i], psink, [i]));
  await mux.outgoing([]).write(w.finish());
  const tx = Promise.all(psink.map((e) => writeDeferred(mux.outgoing(e.path), e))).then(() =>
    mux.closeWrite(),
  );

  const root = mux.incoming([]);
  const { values, sink } = await decodeTuple(root, resultTypes);
  const rx = runReadSink(root, sink);

  const done = Promise.all([tx, rx]).then(() => undefined);
  // Avoid an unhandled rejection if the caller uses `results` without `done`.
  done.catch(() => {});
  return { results: values, done };
}

/**
 * Accept a single invocation on `transport`: read its header and return a
 * handle exposing the invoked `instance`/`func`, methods to receive the
 * parameters and send the results, and the underlying multiplexer.
 *
 * @param {Transport} transport
 */
export async function accept(transport) {
  const mux = new Mux(transport);
  const version = await mux.master.u8();
  if (version !== PROTOCOL) {
    throw new Error(`unsupported wRPC framing version byte 0x${version.toString(16)}`);
  }
  const instance = await mux.master.name();
  const func = await mux.master.name();
  mux.start();
  const root = mux.incoming([]);

  return {
    instance,
    func,
    mux,

    /**
     * Decode the invocation parameters against `paramTypes`. Returns the
     * decoded `params` and a `done` promise for the parameters' async I/O.
     * @param {Type[]} paramTypes
     */
    async receiveParams(paramTypes) {
      const { values, sink } = await decodeTuple(root, paramTypes);
      return { params: values, done: runReadSink(root, sink) };
    },

    /**
     * Encode and send the invocation results, then half-close the write side.
     * Resolves once all asynchronous result I/O has completed.
     * @param {Type[]} resultTypes
     * @param {any[]} values
     */
    async sendResults(resultTypes, values) {
      const w = new Writer();
      /** @type {any[]} */
      const sink = [];
      resultTypes.forEach((ty, i) => encodeValue(w, ty, values[i], sink, [i]));
      await mux.outgoing([]).write(w.finish());
      await Promise.all(sink.map((e) => writeDeferred(mux.outgoing(e.path), e)));
      await mux.closeWrite();
    },
  };
}
