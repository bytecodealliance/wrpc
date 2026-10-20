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

import { Writer } from "./io.js";
import { DISCARDED, Mux, PROTOCOL } from "./mux.js";
import { decodeSteps, encodeValue, hasAsync, readChunk, toBytes } from "./value.js";

/** @typedef {import("./index.d.ts").Type} Type */
/** @typedef {import("./index.d.ts").DecodeDeferred} DecodeDeferred */
/** @typedef {import("./io.js").AsyncReader} AsyncReader */
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
      const bytes = toBytes(chunk);
      // An empty chunk is indistinguishable from the stream terminator.
      if (bytes.length === 0) continue;
      w.varU(bytes.length);
      w.bytes(bytes);
      base += bytes.length;
    } else {
      if (!Array.isArray(chunk)) throw new TypeError("expected an array for a `stream` chunk");
      if (chunk.length === 0) continue;
      w.varU(chunk.length);
      for (let k = 0; k < chunk.length; k++) {
        /** @type {any[]} */
        const sink = [];
        encodeValue(w, elem, chunk[k], sink, []);
        for (const e of sink) {
          const task = writeDeferred(out.index([base + k, ...e.path]), e);
          task.catch(() => {});
          tasks.push(task);
        }
      }
      base += chunk.length;
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
 * @param {AsyncReader} ar
 * @param {Type} ty
 */
function decodeOne(ar, ty) {
  return ar.decode(function* (r) {
    /** @type {DecodeDeferred[]} */
    const sink = [];
    return { value: yield* decodeSteps(r, ty, sink, []), sink };
  });
}

/**
 * Decode a tuple of `types` from the root channel reader, returning the decoded
 * values and the absolute-path descriptors of pending sub-streams.
 * @param {AsyncReader} root
 * @param {Type[]} types
 */
async function decodeTuple(root, types) {
  const values = [];
  /** @type {DecodeDeferred[]} */
  const sink = [];
  for (let i = 0; i < types.length; i++) {
    const { value, sink: nested } = await decodeOne(root, types[i]);
    values.push(value);
    for (const e of nested) sink.push({ ...e, path: [i, ...e.path] });
  }
  return { values, sink };
}

/**
 * Declare to `mux` the sub-streams of the pending values in `sink`, decoded
 * at `path`.
 * @param {Mux} mux @param {number[]} path @param {DecodeDeferred[]} sink
 */
function expect(mux, path, sink) {
  mux.seal(
    path,
    sink.flatMap((e) => (e.kind === "inline" ? [] : [{ path: e.path, nested: hasAsync(e.ty) }])),
  );
}

/**
 * Receive the pending values in `sink`, decoded at `path`. A value at `path`
 * itself continues on `ar`, the reader it was decoded from.
 * @param {Mux} mux @param {number[]} path @param {DecodeDeferred[]} sink
 * @param {AsyncReader} [ar]
 */
function readNested(mux, path, sink, ar) {
  return Promise.all(
    sink.map((e) => readDeferred(mux, [...path, ...e.path], e, e.path.length ? undefined : ar)),
  );
}

/**
 * Receive one pending value at `path` and feed its placeholder.
 * @param {Mux} mux @param {number[]} path @param {DecodeDeferred} entry
 * @param {AsyncReader} [ar]
 * @returns {Promise<unknown>}
 */
async function readDeferred(mux, path, entry, ar) {
  switch (entry.kind) {
    case "inline": {
      // Discard the sub-streams nested in the chunk unless the consumer received it.
      const queue = entry.queue;
      queue.onCancel(() => {
        if (queue.taken === 0) mux.discard(path);
      });
      return;
    }
    case "future":
      return readFuture(mux, path, entry, ar ?? mux.incoming(path));
    case "stream":
      return readStream(mux, path, entry.ty, entry.queue, ar ?? mux.incoming(path));
  }
}

/**
 * @param {Mux} mux @param {number[]} path
 * @param {Extract<DecodeDeferred, { kind: "future" }>} entry @param {AsyncReader} ar
 */
async function readFuture(mux, path, entry, ar) {
  let decoded;
  try {
    decoded = await decodeOne(ar, entry.ty);
  } catch (err) {
    entry.reject(err);
    if (err === DISCARDED) return;
    throw err;
  }
  const { value, sink } = decoded;
  expect(mux, path, sink);
  // A value nested at the future's own path continues reading it.
  if (!sink.some((e) => e.kind !== "inline" && e.path.length === 0)) mux.finish(path);
  const nested = readNested(mux, path, sink, ar);
  entry.resolve(value);
  await nested;
}

/**
 * Receive a stream's chunks over `ar` into `queue`, waiting while it is full.
 * If the consumer cancels the queue, the rest of the stream is discarded,
 * along with the sub-streams nested in the elements it did not receive.
 * @param {Mux} mux @param {number[]} path @param {Type} elem
 * @param {import("./index.d.ts").Chan<any>} queue @param {AsyncReader} ar
 */
async function readStream(mux, path, elem, queue, ar) {
  const nested = hasAsync(elem);
  /** @type {number[]} the index of the first element of each chunk not yet taken */
  const starts = [];
  let taken = 0;
  let base = 0;
  queue.onCancel(() => mux.discard(path, starts[queue.taken - taken] ?? base));
  // Nobody receives the rest of a discarded stream.
  mux.ondiscard(path, () => queue.cancel());
  /** @type {Promise<unknown>[]} */
  const tasks = [];
  try {
    for (;;) {
      // The peer may close the connection instead of sending the empty terminator.
      if (!(await ar.fill(1))) {
        mux.finish(path);
        break;
      }
      const len = Number(await ar.varU(32));
      if (len === 0) {
        mux.finish(path);
        break;
      }
      if (nested) {
        starts.splice(0, queue.taken - taken);
        taken = queue.taken;
        starts.push(base);
      }
      let chunk;
      if (elem.kind === "u8") {
        chunk = await ar.take(len);
      } else if (!nested) {
        chunk = await ar.decode((r) => readChunk(r, elem, len));
      } else {
        chunk = [];
        for (let k = 0; k < len; k++) {
          const { value, sink } = await decodeOne(ar, elem);
          chunk.push(value);
          const at = [...path, base + k];
          expect(mux, at, sink);
          if (!sink.length) continue;
          const task = readNested(mux, at, sink);
          task.catch(() => {});
          tasks.push(task);
        }
      }
      base += len;
      if (!(await queue.send(chunk))) break;
    }
  } catch (err) {
    if (!queue.cancelled && err !== DISCARDED) {
      // Surface a transport/decode failure to whoever is consuming the stream,
      // rather than leaving the consumer hanging on a queue that never closes.
      queue.close(err);
      throw err;
    }
  }
  queue.close();
  await Promise.all(tasks);
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
 * results) has completed. If any of it fails, the transport is aborted.
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
  const w = new Writer();
  /** @type {any[]} */
  const psink = [];
  paramTypes.forEach((ty, i) => encodeValue(w, ty, args[i], psink, [i]));

  const mux = new Mux(transport);
  try {
    await mux.writeHeader(instance, func);
    mux.start();
    await mux.outgoing([]).write(w.finish());
  } catch (err) {
    mux.abort(err);
    throw err;
  }
  const tx = Promise.all(psink.map((e) => writeDeferred(mux.outgoing(e.path), e))).then(() =>
    mux.closeWrite(),
  );
  /** @type {{ err: unknown } | undefined} */
  let txFailure;
  tx.catch((err) => {
    txFailure = { err };
    mux.abort(err);
  });

  const root = mux.incoming([]);
  let decoded;
  try {
    decoded = await decodeTuple(root, resultTypes);
  } catch (err) {
    mux.abort(err);
    throw txFailure ? txFailure.err : err;
  }
  const { values, sink } = decoded;
  expect(mux, [], sink);
  mux.finish([]);
  const rx = readNested(mux, [], sink);
  rx.catch((err) => mux.abort(err));

  const done = Promise.all([tx, rx]).then(() => undefined);
  // Avoid an unhandled rejection if the caller uses `results` without `done`.
  done.catch(() => {});
  return { results: values, done };
}

/**
 * Accept a single invocation on `transport`: read its header and return a
 * handle exposing the invoked `instance`/`func`, methods to receive the
 * parameters and send the results, and the underlying multiplexer. If any
 * I/O fails, the transport is aborted.
 *
 * @param {Transport} transport
 */
export async function accept(transport) {
  const mux = new Mux(transport);
  let instance;
  let func;
  try {
    const version = await mux.master.u8();
    if (version !== PROTOCOL) {
      throw new Error(`unsupported wRPC framing version byte 0x${version.toString(16)}`);
    }
    instance = await mux.master.name();
    func = await mux.master.name();
  } catch (err) {
    mux.abort(err);
    throw err;
  }
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
      let decoded;
      try {
        decoded = await decodeTuple(root, paramTypes);
      } catch (err) {
        mux.abort(err);
        throw err;
      }
      const { values, sink } = decoded;
      expect(mux, [], sink);
      mux.finish([]);
      const done = readNested(mux, [], sink).then(() => undefined);
      done.catch((err) => mux.abort(err));
      return { params: values, done };
    },

    /**
     * Encode and send the invocation results, then half-close the write side.
     * Resolves once all asynchronous result I/O has completed. If the values
     * do not match `resultTypes`, or any I/O fails, the transport is aborted.
     * @param {Type[]} resultTypes
     * @param {any[]} values
     */
    async sendResults(resultTypes, values) {
      try {
        if (values.length !== resultTypes.length) {
          throw new RangeError(`expected ${resultTypes.length} result(s), got ${values.length}`);
        }
        const w = new Writer();
        /** @type {any[]} */
        const sink = [];
        resultTypes.forEach((ty, i) => encodeValue(w, ty, values[i], sink, [i]));
        await mux.outgoing([]).write(w.finish());
        await Promise.all(sink.map((e) => writeDeferred(mux.outgoing(e.path), e)));
        await mux.closeWrite();
      } catch (err) {
        mux.abort(err);
        throw err;
      }
    },
  };
}
