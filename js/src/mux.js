// The framing multiplexer: the layer that turns a transport's single
// bidirectional byte stream into the indexed sub-streams an invocation needs.
//
// Every wRPC transport (TCP, Unix, QUIC, WebTransport, WebSocket, …) provides
// one bidirectional byte stream per invocation; this module implements the
// mandatory wRPC framing on top of it (see `SPEC.md`, "Framed stream
// specification"). Outgoing data is written as frames `[path][data]`, where
// `path` is a `list<u32>` index identifying the (sub-)stream; incoming frames
// are demultiplexed back to the reader registered for their path.

import { AsyncReader, Chan, UNEXPECTED_EOF, Writer, atomic, readVarU32 } from "./io.js";

/** The wRPC framing protocol version byte. */
export const PROTOCOL = 0x00;

/**
 * The number of inbound data chunks buffered per sub-stream. Once a
 * sub-stream's buffer is full, reading from the transport waits for it to be
 * consumed, or for a sub-stream not yet declared, to be declared.
 */
const CAPACITY = 16;

/**
 * The maximum number of sub-streams not yet declared, which data is buffered
 * for. Once reached, reading from the transport waits for them to be declared,
 * so a sub-stream must be declared by data sent before its own.
 */
const MAX_PENDING = 64;

/** The maximum size of a buffered inbound data chunk. */
const MAX_CHUNK = 1 << 20;

/** @param {number[]} path */
const key = (path) => path.join(",");

/** The error a discarded sub-stream's reader fails with. */
export const DISCARDED = new Error("sub-stream discarded");

/**
 * The data buffered for a path that may still be expected.
 * @typedef {object} Pending
 * @property {number[]} path
 * @property {Chan<Uint8Array>} chan
 * @property {boolean} empty whether only empty frames arrived for it
 * @property {boolean} closed whether an empty frame closed it
 * @property {boolean} counted whether it counts towards {@link MAX_PENDING}
 */

/**
 * Where {@link Mux._find} stopped for a path that may still be expected: the
 * node whose child `i` is missing.
 * @typedef {{ at: Node, i: number }} Undecided
 */

/**
 * A node of the trie of expected inbound sub-streams, addressed by path.
 * Once it and the nodes nested in it are finished, it is forgotten: a path
 * missing from a node whose children are all known is no longer expected,
 * like a closed one.
 * @typedef {object} Node
 * @property {Map<number, Node>} children
 * @property {Chan<Uint8Array> | undefined} chan the data of the sub-stream at
 *   this path, if one is expected here
 * @property {boolean} sealed whether every sub-stream nested in it is known
 * @property {number} decided the number of stream elements known, whose
 *   children are all in the trie until forgotten
 * @property {boolean} done whether all its own data has been read
 * @property {boolean} closed whether an empty frame closed it
 * @property {Map<number, Map<string, Pending>> | undefined} pending the data
 *   buffered for the paths through each missing child, by path key
 */

/** @param {boolean} sealed @returns {Node} */
const node = (sealed) => ({
  children: new Map(),
  chan: undefined,
  sealed,
  decided: 0,
  done: false,
  closed: false,
  pending: undefined,
});

/** The child `i` of `n`, created if missing. @param {Node} n @param {number} i */
function child(n, i) {
  let c = n.children.get(i);
  if (!c) n.children.set(i, (c = node(true)));
  return c;
}

/**
 * The channels of `n` and of the nodes nested in it, including those of the
 * data buffered for their missing children.
 * @param {Node} n @returns {Generator<Chan<Uint8Array>>}
 */
function* chans(n) {
  if (n.chan) yield n.chan;
  for (const group of n.pending?.values() ?? []) for (const { chan } of group.values()) yield chan;
  for (const c of n.children.values()) yield* chans(c);
}

/** @param {Undecided} at @param {Pending} pending */
function attach({ at, i }, pending) {
  let group = at.pending?.get(i);
  if (!group) (at.pending ??= new Map()).set(i, (group = new Map()));
  group.set(key(pending.path), pending);
}

/**
 * Read a frame header: the sub-stream path and the data length.
 * @param {import("./io.js").Reader} r
 * @returns {Generator<number, [number[], number], void>}
 */
function* readFrameHeader(r) {
  const path = [];
  for (let depth = yield* atomic(r, readVarU32); depth > 0; depth--) {
    path.push(yield* atomic(r, readVarU32));
  }
  return [path, yield* atomic(r, readVarU32)];
}

/**
 * Abort `transport`, if it can be, ignoring any failure to.
 * @param {Transport} transport @param {unknown} err
 */
function abortTransport(transport, err) {
  Promise.resolve()
    .then(() => transport.abort?.(err))
    .catch(() => {});
}

/** @param {number[]} path */
export const unexpectedError = (path) => new Error(`unexpected data for sub-stream [${path}]`);

/** @param {number[]} path @returns {never} */
function unexpected(path) {
  throw unexpectedError(path);
}

/**
 * A byte-stream transport: a duplex of `Uint8Array` chunks.
 * @typedef {object} Transport
 * @property {() => Promise<Uint8Array | null | undefined>} read next inbound chunk, `null` at EOF
 * @property {(bytes: Uint8Array) => Promise<void> | void} write send an outbound chunk
 * @property {() => Promise<void> | void} closeWrite half-close the write side,
 *   communicating the end of transmit to the peer
 * @property {(reason: unknown) => Promise<void> | void} [abort] tear down both
 *   directions after a failure
 */

/**
 * The application error code a peer stops its receive side with once it is
 * done receiving.
 */
const DONE = 1;

/**
 * Whether `err` reports the send side being stopped with {@link DONE}.
 * @param {any} err
 */
const stoppedDone = (err) => err?.source === "stream" && err?.streamErrorCode === DONE;

/**
 * Adapt a WHATWG stream duplex with a half-close, such as a WebTransport
 * bidirectional stream, to a {@link Transport}, ending transmit by closing
 * the writable side. The peer stopping the writable side with application
 * error code `1` while it is being closed completes the close. A WebSocket
 * has no half-close, so a transport over one must communicate the end of
 * transmit in-band instead.
 * @param {{ readable: ReadableStream<Uint8Array>, writable: WritableStream<Uint8Array> }} duplex
 * @returns {Transport}
 */
export function fromWebStreams({ readable, writable }) {
  const reader = readable.getReader();
  const writer = writable.getWriter();
  return {
    read: async () => {
      const { value, done } = await reader.read();
      return done ? null : value;
    },
    write: (bytes) => writer.write(bytes),
    closeWrite: async () => {
      try {
        await writer.close();
      } catch (err) {
        if (!stoppedDone(err)) throw err;
      }
    },
    abort: async (reason) => {
      const r = stoppedDone(reason) ? undefined : reason;
      await Promise.allSettled([reader.cancel(r), writer.abort(r)]);
    },
  };
}

/**
 * A handle to one (sub-)stream of the multiplexed connection's write side.
 * `write` emits one frame; `index` descends to a nested sub-stream.
 */
export class Outgoing {
  /** @param {Mux} mux @param {number[]} path */
  constructor(mux, path) {
    this.mux = mux;
    this.path = path;
  }

  /**
   * Write `data` as a frame, unless it is empty, since an empty frame closes
   * the sub-stream.
   * @param {Uint8Array} data @returns {Promise<void>}
   */
  write(data) {
    if (data.length > 0) return this.mux._writeFrame(this.path, data);
    const err = this.mux.writeError;
    return err === undefined ? Promise.resolve() : Promise.reject(err);
  }

  /**
   * Close the sub-stream by writing an empty frame. No more data may be
   * written to it afterwards.
   * @returns {Promise<void>}
   */
  close() {
    return this.mux._writeFrame(this.path, new Uint8Array(0));
  }

  /** @param {number[]} rel @returns {Outgoing} */
  index(rel) {
    return new Outgoing(this.mux, [...this.path, ...rel]);
  }
}

/**
 * The framing multiplexer over a {@link Transport}. Construct one per
 * invocation, optionally read the header off `master`, then call `start()` to
 * begin demultiplexing inbound frames.
 *
 * Inbound data is only accepted for the sub-streams the decoded values expect:
 * the root sub-stream `[]`, and those declared with {@link Mux.seal} as each
 * value carrying pending `stream`/`future` values is decoded, until they are
 * {@link Mux.finish finished}. Data for a path that may still be declared is
 * buffered until it is; data for any other path, or for a closed one, fails
 * the invocation, while an empty frame for it is discarded.
 */
export class Mux {
  /**
   * Aborts `transport` and throws a `TypeError` if it does not implement
   * `closeWrite`.
   * @param {Transport} transport
   */
  constructor(transport) {
    if (typeof transport.closeWrite !== "function") {
      const err = new TypeError(
        "a transport must implement `closeWrite` to communicate end of transmit",
      );
      abortTransport(transport, err);
      throw err;
    }
    this.transport = transport;
    /** Reader over the raw inbound byte stream (header, then frames). */
    this.master = new AsyncReader(() => transport.read());
    /** The expected sub-streams, rooted at the one carrying the synchronous values. */
    this.root = node(false);
    this.root.chan = new Chan(CAPACITY);
    /** The number of buffered sub-streams not yet declared. */
    this.pendingCount = 0;
    /** @type {(() => void)[]} */
    this.pendingWaiters = [];
    /** Serializes outbound writes so each frame is emitted atomically. */
    this.writeChain = Promise.resolve();
    /** @type {unknown} */
    this.writeError = undefined;
    this.inboundClosed = false;
    /** @type {unknown} */
    this.inboundError = undefined;
    this.aborted = false;
    /** @type {Promise<void> | undefined} */
    this.ingress = undefined;
    /** @type {Promise<void>} */
    const inbound = new Promise((resolve) => (this.#endInbound = resolve));
    /** @type {Promise<void>} */
    const outbound = new Promise((resolve) => (this.#endOutbound = resolve));
    /** @type {Promise<never>} */
    const failed = new Promise((_, reject) => (this.#fail = reject));
    /**
     * Resolves once the peer ended transmit and every frame it sent was
     * routed, and the write side was half-closed, unless the multiplexer is
     * aborted first: then it rejects with the error it was aborted with. The
     * data routed may still be being decoded.
     * @type {Promise<void>}
     */
    this.ended = Promise.race([Promise.all([inbound, outbound]).then(() => {}), failed]);
    this.ended.catch(() => {});
    /**
     * Rejects with the error the multiplexer is aborted with, once it is.
     * @type {Promise<never>}
     */
    this.failed = failed;
  }

  /** @type {() => void} */
  #endInbound = () => {};
  /** @type {() => void} */
  #endOutbound = () => {};
  /** @type {(err: unknown) => void} */
  #fail = () => {};

  /** Write the invocation header: the protocol byte and instance/function names.
   * @param {string} instance @param {string} func @returns {Promise<void>} */
  writeHeader(instance, func) {
    const w = new Writer();
    w.u8(PROTOCOL);
    w.name(instance);
    w.name(func);
    return this._write(w.finish());
  }

  /** Begin reading and routing inbound frames off `master`. */
  start() {
    if (this.ingress) return;
    this.ingress = this._ingress().then(
      () => this.#endInbound(),
      (err) => this.abort(err),
    );
  }

  async _ingress() {
    const m = this.master;
    while (!this.aborted) {
      // Stop cleanly at a frame boundary once the peer closes the stream.
      if (!(await m.fill(1))) break;
      const [path, n] = await m.decode(readFrameHeader);
      await this._room(path, n === 0);
      if (n === 0) {
        this._route(path, true)?.close();
        continue;
      }
      const chan = this._route(path);
      for (let len = n; len > 0;) {
        if (!(await m.fill(1))) throw new RangeError(UNEXPECTED_EOF);
        const data = m.remaining().slice(0, Math.min(len, MAX_CHUNK));
        m.consume(data.length);
        len -= data.length;
        // A finished or discarded sub-stream's reader is gone: the rest of the frame overruns it.
        if (chan && !(await chan.send(data)) && !this.aborted) unexpected(path);
      }
    }
    this._closeInbound();
  }

  /**
   * Wait while {@link MAX_PENDING} sub-streams not yet declared are buffered,
   * if the frame at `path` would buffer another one.
   * @param {number[]} path
   * @param {boolean} empty
   */
  async _room(path, empty) {
    while (this.pendingCount >= MAX_PENDING && !this.aborted) {
      const found = this._find(path, empty);
      if (found === null || !("at" in found)) return;
      if (found.at.pending?.get(found.i)?.has(key(path))) return;
      await new Promise((resolve) => this.pendingWaiters.push(() => resolve(undefined)));
    }
  }

  /** Wake the frame waiting in {@link Mux._room}, to look its path up again. */
  _wake() {
    for (const f of this.pendingWaiters.splice(0)) f();
  }

  /** Stop counting `pending` towards {@link MAX_PENDING}. @param {Pending} pending */
  _uncount(pending) {
    if (!pending.counted) return;
    pending.counted = false;
    this.pendingCount--;
    this._wake();
  }

  /**
   * The node of the sub-stream at `path`, or where the lookup stopped if it
   * is not yet known whether it is expected. A path no sub-stream is or may
   * still be expected at, or a closed or finished one, fails the invocation,
   * unless `empty`, for an empty frame, which is to be discarded: then it
   * gives `null`.
   * @param {number[]} path
   * @param {boolean} [empty]
   * @returns {Node | null | Undecided}
   */
  _find(path, empty = false) {
    const fail = () => (empty ? null : unexpected(path));
    let n = this.root;
    for (const i of path) {
      const c = n.children.get(i);
      if (!c) return n.sealed || i < n.decided ? fail() : { at: n, i };
      n = c;
    }
    return n.closed || n.done || !n.chan ? fail() : n;
  }

  /**
   * The channel to route the data of the sub-stream at `path` to, or
   * `undefined` to drop it. With `empty`, for an empty frame, the sub-stream
   * is marked closed, and the data of a path that may still be expected is
   * buffered as if it were finished.
   * @param {number[]} path
   * @param {boolean} [empty]
   */
  _route(path, empty = false) {
    const found = this._find(path, empty);
    if (found === null) return undefined;
    if (!("at" in found)) {
      found.closed ||= empty;
      return found.chan;
    }
    let pending = found.at.pending?.get(found.i)?.get(key(path));
    if (pending?.closed) return empty ? undefined : unexpected(path);
    if (!pending) {
      const p = (pending = { path, chan: new Chan(CAPACITY), empty, closed: false, counted: true });
      this.pendingCount++;
      p.chan.onCancel(() => this._uncount(p));
      attach(found, p);
    }
    pending.empty &&= empty;
    pending.closed ||= empty;
    return pending.chan;
  }

  /**
   * Take the data buffered for `path` before it was declared, if any, and
   * name the group it was in, to settle once the path's nodes exist.
   * @param {number[]} path
   * @returns {(Undecided & { pending: Pending | undefined }) | undefined}
   */
  _takePending(path) {
    let n = this.root;
    for (const i of path) {
      const c = n.children.get(i);
      if (!c) {
        const group = n.pending?.get(i);
        const k = key(path);
        const pending = group?.get(k);
        group?.delete(k);
        if (pending) this._uncount(pending);
        return { at: n, i, pending };
      }
      n = c;
    }
    return undefined;
  }

  /**
   * The node at `path`, `undefined` if there is none.
   * @param {number[]} path
   */
  _get(path) {
    let n = this.root;
    for (const i of path) {
      const c = n.children.get(i);
      if (!c) return undefined;
      n = c;
    }
    return n;
  }

  /**
   * Declare the sub-streams nested in the value at `path` once it is decoded:
   * the root tuple at `[]`, a future's value at its own path, or element `k`
   * of a stream at `[...stream, k]`, decoded in order. `expected` lists the
   * paths, relative to `path`, of the value's pending `stream`/`future`
   * values, and whether each may nest sub-streams of its own. Data for any
   * other path nested in the value fails the invocation. If the value was
   * forgotten, as discarded ones are, nothing is declared: the data buffered
   * for the sub-streams is discarded, and any of it fails the invocation.
   * @param {number[]} path
   * @param {{ path: number[], nested: boolean }[]} expected
   * @returns {boolean} whether the sub-streams are declared
   */
  seal(path, expected) {
    /** @type {Undecided[]} */
    const settle = [];
    const taken = expected.map((e) => this._takePending([...path, ...e.path]));
    for (const t of taken) if (t) settle.push(t);
    let scope = this._get(path);
    const parent = path.length ? this._get(path.slice(0, -1)) : undefined;
    if (parent && !parent.sealed) {
      const k = path[path.length - 1];
      const from = parent.decided;
      parent.decided = Math.max(from, k + 1);
      for (let i = from; i < parent.decided; i++) settle.push({ at: parent, i });
      if (!scope && expected.length && k >= from) {
        scope = child(parent, k);
      }
    }
    if (scope) {
      scope.sealed = true;
      for (const [j, e] of expected.entries()) this._declare(scope, e, taken[j]?.pending);
      for (const i of scope.pending?.keys() ?? []) settle.push({ at: scope, i });
    } else {
      this._drop(
        path,
        taken.flatMap((t) => (t?.pending ? [t.pending.chan] : [])),
      );
    }
    for (const at of settle) this._settle(at);
    this._wake();
    return !!scope || !expected.length;
  }

  /**
   * Declare the sub-stream of a pending value at `e.path` relative to `scope`,
   * taking over the data buffered for it.
   * @param {Node} scope
   * @param {{ path: number[], nested: boolean }} e
   * @param {Pending | undefined} pending
   */
  _declare(scope, e, pending) {
    let n = scope;
    for (const i of e.path) n = child(n, i);
    n.sealed = !e.nested;
    if (n.chan) return;
    n.chan = pending?.chan ?? new Chan(CAPACITY);
    n.closed = pending?.closed ?? false;
    if (this.inboundClosed) n.chan.close(this.inboundError);
  }

  /**
   * Declare that the sub-stream at `path` has no more data: its stream ended,
   * or its future's value was decoded. Any further data for it fails the
   * invocation, as does any data of it left unread in `reader`. Once the
   * sub-streams nested in it finish too, it is forgotten.
   * @param {number[]} path
   * @param {AsyncReader} [reader] the reader it was read over
   */
  finish(path, reader) {
    const n = this._get(path);
    if (reader?.remaining().length || n?.chan?.queue.length) {
      this.abort(unexpectedError(path));
    }
    if (!n) return;
    n.sealed = true;
    n.done = true;
    n.chan?.close();
    for (const i of [...(n.pending?.keys() ?? [])]) this._settle({ at: n, i });
    this._prune(path);
    this._wake();
  }

  /**
   * Forget the node at `path` and its ancestors as long as they are finished
   * and without children.
   * @param {number[]} path
   */
  _prune(path) {
    const nodes = [this.root];
    for (const i of path) {
      const c = nodes[nodes.length - 1].children.get(i);
      if (!c) return;
      nodes.push(c);
    }
    for (let d = path.length; d > 0; d--) {
      const n = nodes[d];
      if (n.children.size || (n.chan && !n.done)) return;
      nodes[d - 1].children.delete(path[d - 1]);
    }
  }

  /**
   * Discard the sub-streams nested in elements `from` up to `to` of the
   * stream at `path`: fail their readers with {@link DISCARDED} and forget
   * them, so that any further data for them fails the invocation, as does any
   * of their data left unread.
   * @param {number[]} path @param {number} from @param {number} to
   */
  discard(path, from, to) {
    const n = this._get(path);
    if (!n) return;
    for (const [k, c] of n.children) {
      if (k < from || k >= to) continue;
      n.children.delete(k);
      this._drop([...path, k], chans(c));
    }
    this._prune(path);
    this._wake();
  }

  /**
   * Call `f` once the sub-stream expected at `path` is discarded, or now if
   * it already is.
   * @param {number[]} path @param {() => void} f
   */
  onDiscard(path, f) {
    this._get(path)?.chan?.onCancel(f);
  }

  /**
   * Fail the readers of `dropped` with {@link DISCARDED}, aborting the
   * invocation if any of them holds data, as data for the sub-streams nested
   * in the value at `path`.
   * @param {number[]} path @param {Iterable<Chan<Uint8Array>>} dropped
   */
  _drop(path, dropped) {
    let unread = false;
    for (const chan of dropped) {
      unread ||= chan.queue.length > 0;
      chan.cancel(DISCARDED);
    }
    if (unread) this.abort(unexpectedError(path));
  }

  /**
   * Look up the paths buffered in the group at `at` again: drop those decided
   * since, failing the invocation on any data of theirs, and keep the rest
   * where their lookup stops now.
   * @param {Undecided} at
   */
  _settle(at) {
    const group = at.at.pending?.get(at.i);
    if (!group) return;
    at.at.pending?.delete(at.i);
    for (const pending of group.values()) {
      let found;
      try {
        found = this._find(pending.path, pending.empty);
      } catch (err) {
        this.abort(err);
        return;
      }
      if (found === null || !("at" in found)) this._drop(pending.path, [pending.chan]);
      else attach(found, pending);
    }
  }

  /** @param {unknown} [err] */
  _closeInbound(err) {
    if (this.inboundClosed) return;
    this.inboundClosed = true;
    this.inboundError = err;
    for (const chan of chans(this.root)) chan.close(err);
  }

  /**
   * The reader for the expected sub-stream at `path` (the root is `[]`).
   * @param {number[]} path
   * @returns {AsyncReader}
   */
  incoming(path) {
    const chan = this._get(path)?.chan;
    if (!chan) throw new Error(`no sub-stream is expected at [${path}]`);
    return new AsyncReader(async () => {
      const { value, done } = await chan.next();
      return done ? null : value;
    });
  }

  /** The writer for the sub-stream addressed by `path` (root is `[]`).
   * @param {number[]} path @returns {Outgoing} */
  outgoing(path) {
    return new Outgoing(this, path);
  }

  /** @param {number[]} path @param {Uint8Array} data */
  _writeFrame(path, data) {
    const w = new Writer();
    w.varU(path.length);
    for (const p of path) w.varU(p);
    w.varU(data.length);
    w.bytes(data);
    return this._write(w.finish());
  }

  /** @param {Uint8Array} bytes */
  _write(bytes) {
    if (this.writeError !== undefined) return Promise.reject(this.writeError);
    this.writeChain = this.writeChain.then(() => this.transport.write(bytes));
    return this.writeChain;
  }

  /**
   * Abort the invocation: fail every subsequent write and pending or future
   * read with `err`, and abort the transport.
   * @param {unknown} [err]
   */
  abort(err = new Error("invocation aborted")) {
    if (this.aborted) return;
    this.aborted = true;
    this.writeError = err;
    this._closeInbound(err);
    this._wake();
    this.inboundError ??= err;
    this.#fail(err);
    abortTransport(this.transport, err);
  }

  /**
   * Flush pending writes and half-close the write side, communicating the end
   * of transmit to the peer. @returns {Promise<void>}
   */
  async closeWrite() {
    await this.writeChain;
    if (this.writeError !== undefined) throw this.writeError;
    await this.transport.closeWrite();
    this.#endOutbound();
  }
}
