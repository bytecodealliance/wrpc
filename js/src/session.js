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

import { Writer, atomic, readVarU32 } from "./io.js";
import { DISCARDED, Mux, PROTOCOL, unexpectedError } from "./mux.js";
import {
  asyncDepth,
  decodeSteps,
  encodeValue,
  hasAsync,
  isTypedArray,
  readChunk,
  toList,
  typedArray,
} from "./value.js";

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
 * Yield the chunks of a `stream` source. A stream of a numeric type may be
 * given as a single typed array of it; otherwise the source is any
 * (async-)iterable of chunks (arrays of elements, or typed arrays for numeric
 * types).
 * @param {any} src
 * @param {Type} elem
 */
async function* streamChunks(src, elem) {
  if (src === null || src === undefined) return;
  const typed = typedArray(elem);
  if (typed && isTypedArray(src, typed)) {
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

/** @param {Outgoing} out @param {Type | null} ty @param {any} value */
async function writeFuture(out, ty, value) {
  const v = await resolveFuture(value);
  const w = new Writer();
  /** @type {any[]} */
  const sink = [];
  if (ty) encodeValue(w, ty, v, sink, []);
  const bytes = w.finish();
  await (bytes.length > 0 ? out.write(bytes) : out.close());
  await Promise.all(sink.map((e) => writeDeferred(out.index(e.path), e)));
}

/**
 * The tasks transmitting or receiving the values nested in stream elements,
 * forgotten once they succeed.
 */
class Tasks {
  /** @type {Set<Promise<unknown>>} */
  #pending = new Set();
  /** @type {(err: unknown) => void} */
  #fail = () => {};
  /** Rejects once any task fails. @type {Promise<never>} */
  failed = new Promise((_, reject) => (this.#fail = reject));

  constructor() {
    this.failed.catch(() => {});
  }

  /** @param {Promise<unknown>} task */
  add(task) {
    this.#pending.add(task);
    task.then(
      () => this.#pending.delete(task),
      (err) => this.#fail(err),
    );
  }

  /**
   * `p`, unless a task fails first.
   * @template T @param {Promise<T>} p @returns {Promise<T>}
   */
  race(p) {
    return Promise.race([p, this.failed]);
  }

  /** Wait for the tasks not succeeded yet. */
  wait() {
    return Promise.all(this.#pending);
  }
}

/** @param {Outgoing} out @param {Type} elem @param {any} src */
async function writeStream(out, elem, src) {
  let base = 0;
  const tasks = new Tasks();
  const chunks = streamChunks(src, elem);
  try {
    for (;;) {
      const next = await tasks.race(chunks.next());
      if (next.done) break;
      base += await writeChunk(out, elem, next.value, base, tasks);
    }
  } finally {
    chunks.return(undefined).catch(() => {});
  }
  const end = new Writer();
  end.varU(0); // a stream is terminated by an empty chunk
  await tasks.race(out.write(end.finish()));
  await tasks.wait();
}

/**
 * Transmit one `stream` chunk over `out`, whose first element is at `base`,
 * then the values nested in its elements, and return its length.
 * @param {Outgoing} out @param {Type} elem @param {any} chunk @param {number} base
 * @param {Tasks} tasks
 */
async function writeChunk(out, elem, chunk, base, tasks) {
  const w = new Writer();
  const elems = toList(chunk, elem);
  // An empty chunk is indistinguishable from the stream terminator.
  if (elems.length === 0) return 0;
  w.varU(elems.length);
  if (elem.kind === "u8") {
    w.bytes(/** @type {Uint8Array} */ (elems));
    await tasks.race(out.write(w.finish()));
    return elems.length;
  }
  /** @type {[number[], any][]} */
  const deferred = [];
  for (let k = 0; k < elems.length; k++) {
    /** @type {any[]} */
    const sink = [];
    encodeValue(w, elem, elems[k], sink, []);
    for (const e of sink) deferred.push([[base + k, ...e.path], e]);
  }
  await tasks.race(out.write(w.finish()));
  for (const [path, e] of deferred) tasks.add(writeDeferred(out.index(path), e));
  return elems.length;
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
 * Receive a tuple of `types` from the root channel reader: the decoded
 * `values`, and a `done` promise for the pending sub-streams they carry. If
 * any of it fails, `mux` is aborted.
 * @param {Mux} mux
 * @param {AsyncReader} root
 * @param {Type[]} types
 */
async function receiveTuple(mux, root, types) {
  const values = [];
  /** @type {DecodeDeferred[]} */
  const sink = [];
  try {
    for (let i = 0; i < types.length; i++) {
      const { value, sink: nested } = await decodeOne(root, types[i]);
      values.push(value);
      for (const e of nested) sink.push({ ...e, path: [i, ...e.path] });
    }
  } catch (err) {
    mux.abort(err);
    throw err;
  }
  expect(mux, [], sink);
  mux.finish([], root);
  const done = unlessAborted(mux, readNested(mux, [], sink));
  done.catch((err) => mux.abort(err));
  return { values, done };
}

/**
 * Resolves once `p` resolves, unless `mux` was aborted by then: then it
 * rejects with the error it was aborted with.
 * @param {Mux} mux @param {Promise<unknown>} p @returns {Promise<void>}
 */
async function unlessAborted(mux, p) {
  await Promise.race([p, mux.failed]);
  if (mux.aborted) throw mux.writeError;
}

/**
 * Declare to `mux` the sub-streams the root values of `types` can carry.
 * @param {Mux} mux @param {Type[]} types
 */
function restrict(mux, types) {
  mux.restrict(
    types.flatMap((ty, i) => (hasAsync(ty) ? [i] : [])),
    Math.max(0, ...types.map((ty) => asyncDepth(ty) + 1)),
  );
}

/**
 * Declare to `mux` the sub-streams of the pending values in `sink`, decoded
 * at `path`, unless the value was discarded.
 * @param {Mux} mux @param {number[]} path @param {DecodeDeferred[]} sink
 * @returns {boolean} whether they are declared
 */
function expect(mux, path, sink) {
  return mux.seal(
    path,
    sink.flatMap((e) =>
      e.kind === "inline" ? [] : [{ path: e.path, nested: !!e.ty && hasAsync(e.ty) }],
    ),
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
    case "inline":
      entry.queue.onCancel(([chunk]) => {
        if (chunk) mux.discard(path, 0, chunk.length);
      });
      return;
    case "future":
      return readFuture(mux, path, entry, ar ?? mux.incoming(path));
    case "stream":
      mux.onDiscard(path, () => entry.queue.cancel(DISCARDED));
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
    if (entry.ty) decoded = await decodeOne(ar, entry.ty);
    else if (await ar.fill(1)) throw unexpectedError(path);
    else decoded = { value: undefined, sink: [] };
  } catch (err) {
    if (err !== DISCARDED) throw err;
  }
  if (!decoded || !expect(mux, path, decoded.sink)) {
    if (ar.remaining().length) throw unexpectedError(path);
    return;
  }
  const { value, sink } = decoded;
  // A value nested at the future's own path continues reading it.
  if (!sink.some((e) => e.kind !== "inline" && e.path.length === 0)) mux.finish(path, ar);
  const nested = readNested(mux, path, sink, ar);
  entry.resolve(value);
  await nested;
}

/**
 * Receive a stream's chunks over `ar` into `queue`, waiting while it is full.
 * If the consumer cancels the queue, the sub-streams nested in the elements
 * of the chunks it dropped are discarded, while those of the chunks it
 * received keep receiving, and any further chunk fails the invocation. The
 * queue of a stream discarded along with its parent is cancelled with
 * {@link DISCARDED}, dropping the chunks nobody can receive.
 * @param {Mux} mux @param {number[]} path @param {Type} elem
 * @param {import("./index.d.ts").Chan<any>} queue @param {AsyncReader} ar
 */
async function readStream(mux, path, elem, queue, ar) {
  const nested = hasAsync(elem);
  /** The index of the first element of each chunk sent. @type {WeakMap<any[], number>} */
  const starts = new WeakMap();
  if (nested) {
    queue.onCancel((dropped) => {
      if (!dropped.length) return;
      const from = /** @type {number} */ (starts.get(dropped[0]));
      mux.discard(path, from, from + dropped.reduce((n, chunk) => n + chunk.length, 0));
    });
  }
  let base = 0;
  const tasks = new Tasks();
  try {
    for (;;) {
      const len = await tasks.race(ar.decode((r) => atomic(r, readVarU32)));
      if (len === 0) {
        mux.finish(path, ar);
        break;
      }
      if (queue.cancelled) throw unexpectedError(path);
      let chunk;
      if (elem.kind === "u8") {
        chunk = await tasks.race(ar.take(len));
      } else if (!nested) {
        chunk = await tasks.race(ar.decode((r) => readChunk(r, elem, len)));
      } else {
        chunk = [];
        for (let k = 0; k < len; k++) {
          const { value, sink } = await tasks.race(decodeOne(ar, elem));
          chunk.push(value);
          const at = [...path, base + k];
          if (!expect(mux, at, sink) || !sink.length) continue;
          tasks.add(readNested(mux, at, sink));
        }
        starts.set(chunk, base);
      }
      base += len;
      if (!(await tasks.race(queue.send(chunk)))) {
        throw queue.error === DISCARDED ? DISCARDED : unexpectedError(path);
      }
    }
  } catch (err) {
    if (err !== DISCARDED) throw err;
    if (ar.remaining().length) throw unexpectedError(path);
    await tasks.wait();
    return;
  }
  queue.close();
  await tasks.wait();
}

// ----- public API -----------------------------------------------------------

/**
 * Resolves once `mux` ended transmit both ways and `decoded`, the decoding of
 * all the data received, resolved; rejects with any failure of the invocation.
 * @param {Mux} mux @param {Promise<unknown>} decoded
 * @returns {Promise<void>}
 */
function finished(mux, decoded) {
  const p = unlessAborted(mux, Promise.all([mux.ended, decoded]));
  p.catch(() => {});
  return p;
}

/**
 * Invoke `func` on `instance` over `transport`, sending `args` (encoded against
 * `paramTypes`) and decoding the reply against `resultTypes`.
 *
 * Returns the decoded `results` immediately once the synchronous reply frame
 * arrives — `stream` results are async iterables of chunks and `future`
 * results are promises, both fed as their data arrives — together with a
 * `done` promise that resolves once all asynchronous I/O (parameters and
 * results) has completed, and a `finished` promise that resolves once the
 * peer has also ended transmit and all the data it sent has been decoded,
 * reporting any failure after `done` too. If the arguments do not match
 * `paramTypes`, or any of it fails, the transport is aborted.
 *
 * @param {Transport} transport
 * @param {string} instance
 * @param {string} func
 * @param {Type[]} paramTypes
 * @param {any[]} args
 * @param {Type[]} [resultTypes]
 * @returns {Promise<{ results: any[], done: Promise<void>, finished: Promise<void> }>}
 */
export async function invoke(transport, instance, func, paramTypes, args, resultTypes = []) {
  const mux = new Mux(transport);
  const w = new Writer();
  /** @type {any[]} */
  const psink = [];
  try {
    if (args.length !== paramTypes.length) {
      throw new RangeError(`expected ${paramTypes.length} argument(s), got ${args.length}`);
    }
    paramTypes.forEach((ty, i) => encodeValue(w, ty, args[i], psink, [i]));
    await mux.writeHeader(instance, func);
    restrict(mux, resultTypes);
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

  let received;
  try {
    received = await receiveTuple(mux, mux.incoming([]), resultTypes);
  } catch (err) {
    throw txFailure ? txFailure.err : err;
  }
  const { values, done: rx } = received;

  const done = unlessAborted(mux, Promise.all([tx, rx]));
  // Avoid an unhandled rejection if the caller uses `results` without `done`.
  done.catch(() => {});
  return { results: values, done, finished: finished(mux, done) };
}

/**
 * Accept a single invocation on `transport`: read its header and return a
 * handle exposing the invoked `instance`/`func`, methods to receive the
 * parameters and send the results, the underlying multiplexer, and a
 * `finished` promise that resolves once the parameters have been received,
 * the peer has ended transmit, the results have been sent and all the data the
 * peer sent has been decoded, reporting any failure after the parameters'
 * `done` too. If any I/O fails, the transport is aborted.
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
  const root = mux.incoming([]);
  /** @type {(received: ReturnType<typeof receiveTuple>) => void} */
  let receive = () => {};
  /** @type {ReturnType<typeof receiveTuple>} */
  const received = new Promise((resolve) => (receive = resolve));

  return {
    instance,
    func,
    mux,
    finished: finished(
      mux,
      received.then(({ done }) => done),
    ),

    /**
     * Decode the invocation parameters against `paramTypes`. Returns the
     * decoded `params` and a `done` promise for the parameters' async I/O.
     * @param {Type[]} paramTypes
     */
    async receiveParams(paramTypes) {
      restrict(mux, paramTypes);
      mux.start();
      const receiving = receiveTuple(mux, root, paramTypes);
      receive(receiving);
      const { values, done } = await receiving;
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
