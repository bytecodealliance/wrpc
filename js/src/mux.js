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
 * consumed.
 */
const CAPACITY = 16;

/** @param {number[]} path */
const key = (path) => path.join(",");

/** The error a discarded sub-stream's reader fails with. */
export const DISCARDED = new Error("sub-stream discarded");

/** {@link Mux._find}'s result for a path that may still be expected. */
const UNDECIDED = Symbol("undecided");

/**
 * A node of the trie of expected inbound sub-streams, addressed by path.
 * @typedef {object} Node
 * @property {Map<number, Node>} children
 * @property {Chan<Uint8Array> | undefined} chan the data of the sub-stream at
 *   this path, if one is expected here
 * @property {boolean} sealed whether every sub-stream nested in it is known
 * @property {number} decided the number of stream elements known, whose
 *   children are all in the trie
 * @property {boolean} done whether all its own data has been read
 * @property {Set<number> | undefined} pruned the indices of the children
 *   forgotten once finished, beyond those of the `decided` stream elements
 * @property {number} discardFrom `Infinity`, unless the node is discarded: then
 *   its own data and that of its children from this index on is dropped
 */

/** @param {boolean} sealed @returns {Node} */
const node = (sealed) => ({
  children: new Map(),
  chan: undefined,
  sealed,
  decided: 0,
  done: false,
  pruned: undefined,
  discardFrom: Infinity,
});

/** @param {Node} n @param {(chan: Chan<Uint8Array>) => void} f */
function eachChan(n, f) {
  if (n.chan) f(n.chan);
  for (const child of n.children.values()) eachChan(child, f);
}

/** @param {number[]} path @returns {never} */
function unexpected(path) {
  throw new Error(`unexpected data for sub-stream [${path}]`);
}

/**
 * A byte-stream transport: a duplex of `Uint8Array` chunks.
 * @typedef {object} Transport
 * @property {() => Promise<Uint8Array | null | undefined>} read next inbound chunk, `null` at EOF
 * @property {(bytes: Uint8Array) => Promise<void> | void} write send an outbound chunk
 * @property {() => Promise<void> | void} [closeWrite] half-close the write side
 * @property {(reason: unknown) => Promise<void> | void} [abort] tear down both
 *   directions after a failure
 */

/**
 * Adapt a WHATWG stream duplex (as provided by a WebTransport bidirectional
 * stream, or built over a WebSocket / `MessagePort`) to a {@link Transport}.
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
    closeWrite: () => writer.close(),
    abort: async (reason) => {
      await Promise.allSettled([reader.cancel(reason), writer.abort(reason)]);
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

  /** @param {Uint8Array} data @returns {Promise<void>} */
  write(data) {
    return this.mux._writeFrame(this.path, data);
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
 * buffered until it is; data for any other path fails the invocation.
 */
export class Mux {
  /** @param {Transport} transport */
  constructor(transport) {
    this.transport = transport;
    /** Reader over the raw inbound byte stream (header, then frames). */
    this.master = new AsyncReader(() => transport.read());
    /** The expected sub-streams, rooted at the one carrying the synchronous values. */
    this.root = node(false);
    this.root.chan = new Chan(CAPACITY);
    /** @type {Map<string, { path: number[], chan: Chan<Uint8Array>, empty: boolean }>} data for undecided paths */
    this.pending = new Map();
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
  }

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
    this.ingress = this._ingress().catch((err) => this.abort(err));
  }

  async _ingress() {
    const m = this.master;
    while (!this.aborted) {
      // Stop cleanly at a frame boundary once the peer closes the stream.
      if (!(await m.fill(1))) break;
      const [path, n] = await m.decode(function* (r) {
        const path = [];
        for (let depth = yield* atomic(r, readVarU32); depth > 0; depth--) {
          path.push(yield* atomic(r, readVarU32));
        }
        return [path, yield* atomic(r, readVarU32)];
      });
      if (n === 0) {
        // Peers may send empty frames, even for sub-streams that are finished.
        if (this._find(path, true) === UNDECIDED) this._route(path, true);
        continue;
      }
      const chan = this._route(path);
      for (let len = n; len > 0; ) {
        if (!(await m.fill(1))) throw new RangeError(UNEXPECTED_EOF);
        const data = m.remaining().slice(0, len);
        m.consume(data.length);
        len -= data.length;
        if (chan) await chan.send(data);
      }
    }
    this._closeInbound();
  }

  /**
   * The channel of the sub-stream at `path`, `null` if its data is discarded,
   * or {@link UNDECIDED} if it is not yet known whether it is expected. With
   * `empty`, for an empty frame, a finished sub-stream also gives `null`.
   * @param {number[]} path
   * @param {boolean} [empty]
   * @returns {Chan<Uint8Array> | null | typeof UNDECIDED}
   */
  _find(path, empty = false) {
    let n = this.root;
    for (const i of path) {
      if (i >= n.discardFrom) return null;
      const child = n.children.get(i);
      if (!child) {
        const finished = i < n.decided || !!n.pruned?.has(i);
        if (empty && finished) return null;
        return n.sealed || finished ? unexpected(path) : UNDECIDED;
      }
      n = child;
    }
    if (n.done) return empty ? null : unexpected(path);
    if (n.discardFrom !== Infinity) return null;
    return n.chan ?? (n.sealed ? unexpected(path) : UNDECIDED);
  }

  /**
   * The channel to route the data of the sub-stream at `path` to, or
   * `undefined` to drop it. With `empty`, for an empty frame, the data of a
   * path that may still be expected is buffered as if it were finished.
   * @param {number[]} path
   * @param {boolean} [empty]
   */
  _route(path, empty = false) {
    const found = this._find(path, empty);
    if (found === null) return undefined;
    if (found !== UNDECIDED) return found;
    const k = key(path);
    let pending = this.pending.get(k);
    if (!pending) {
      pending = { path, chan: new Chan(CAPACITY), empty };
      this.pending.set(k, pending);
    }
    pending.empty &&= empty;
    return pending.chan;
  }

  /**
   * The node at `path`, `undefined` if there is none, or `null` if it is
   * discarded by an ancestor. With `create`, missing nodes are created.
   * @param {number[]} path
   * @param {boolean} [create]
   */
  _get(path, create = false) {
    let n = this.root;
    for (const i of path) {
      if (i >= n.discardFrom) return null;
      let child = n.children.get(i);
      if (!child) {
        if (!create) return undefined;
        child = node(true);
        n.children.set(i, child);
      }
      n = child;
    }
    return n;
  }

  /**
   * Declare the sub-streams nested in the value at `path` once it is decoded:
   * the root tuple at `[]`, a future's value at its own path, or element `k`
   * of a stream at `[...stream, k]`, decoded in order. `expected` lists the
   * paths, relative to `path`, of the value's pending `stream`/`future`
   * values, and whether each may nest sub-streams of its own. Data for any
   * other path nested in the value fails the invocation.
   * @param {number[]} path
   * @param {{ path: number[], nested: boolean }[]} expected
   */
  seal(path, expected) {
    if (path.length) {
      const parent = this._get(path.slice(0, -1));
      if (parent) parent.decided = Math.max(parent.decided, path[path.length - 1] + 1);
    }
    const scope = this._get(path, expected.length > 0);
    if (scope) scope.sealed = true;
    for (const e of expected) {
      const abs = [...path, ...e.path];
      const n = this._get(abs, true);
      if (!n) continue;
      n.sealed = !e.nested;
      if (n.chan) continue;
      const k = key(abs);
      n.chan = this.pending.get(k)?.chan ?? new Chan(CAPACITY);
      this.pending.delete(k);
      if (this.inboundClosed) n.chan.close(this.inboundError);
    }
    this._settle();
  }

  /**
   * Declare that the sub-stream at `path` has no more data: its stream ended,
   * or its future's value was decoded. Once the sub-streams nested in it
   * finish too, it is forgotten, and any further data for it fails the
   * invocation.
   * @param {number[]} path
   */
  finish(path) {
    const n = this._get(path);
    if (!n) return;
    n.sealed = true;
    n.done = true;
    this._prune(path);
    this._settle();
  }

  /**
   * Forget the node at `path` and its ancestors as long as they are finished,
   * without children, and not discarded.
   * @param {number[]} path
   */
  _prune(path) {
    const nodes = [this.root];
    for (const i of path) {
      const child = nodes[nodes.length - 1].children.get(i);
      if (!child) return;
      nodes.push(child);
    }
    for (let d = path.length; d > 0; d--) {
      const n = nodes[d];
      if (n.children.size || n.discardFrom !== Infinity || (n.chan && !n.done)) return;
      const parent = nodes[d - 1];
      const i = path[d - 1];
      parent.children.delete(i);
      if (i >= parent.decided) (parent.pruned ??= new Set()).add(i);
    }
  }

  /**
   * Discard the sub-stream at `path` and those nested in its children from
   * index `from` on: drop their buffered and future data, and fail their
   * readers with {@link DISCARDED}.
   * @param {number[]} path
   * @param {number} [from]
   */
  discard(path, from = 0) {
    const n = this._get(path);
    if (!n || (n.done && ![...n.children.keys()].some((i) => i >= from))) return;
    n.discardFrom = Math.min(n.discardFrom, from);
    n.chan?.cancel(DISCARDED);
    for (const [i, child] of n.children) {
      if (i < from) continue;
      eachChan(child, (chan) => chan.cancel(DISCARDED));
      n.children.delete(i);
    }
    this._settle();
  }

  /**
   * Call `f` once the sub-stream at `path` is discarded.
   * @param {number[]} path @param {() => void} f
   */
  ondiscard(path, f) {
    const chan = this._get(path)?.chan;
    if (chan) chan.onCancel(f);
    else f();
  }

  /** Drop or reject the buffered data of the paths decided since it arrived. */
  _settle() {
    for (const [k, { path, chan, empty }] of this.pending) {
      let found;
      try {
        found = this._find(path, empty);
      } catch (err) {
        this.abort(err);
        return;
      }
      if (found === UNDECIDED) continue;
      this.pending.delete(k);
      chan.cancel(DISCARDED);
    }
  }

  /** @param {unknown} [err] */
  _closeInbound(err) {
    if (this.inboundClosed) return;
    this.inboundClosed = true;
    this.inboundError = err;
    eachChan(this.root, (chan) => chan.close(err));
    for (const { chan } of this.pending.values()) chan.close(err);
  }

  /**
   * The reader for the expected sub-stream at `path` (the root is `[]`). The
   * reader of a discarded sub-stream fails with {@link DISCARDED}.
   * @param {number[]} path
   * @returns {AsyncReader}
   */
  incoming(path) {
    const n = this._get(path);
    if (n !== null && !n?.chan) throw new Error(`no sub-stream is expected at [${path}]`);
    const chan = n?.chan;
    return new AsyncReader(async () => {
      if (!chan) throw DISCARDED;
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
    this.inboundError ??= err;
    Promise.resolve()
      .then(() => this.transport.abort?.(err))
      .catch(() => {});
  }

  /** Flush pending writes and half-close the write side. @returns {Promise<void>} */
  async closeWrite() {
    await this.writeChain;
    if (this.writeError !== undefined) throw this.writeError;
    if (this.transport.closeWrite) await this.transport.closeWrite();
  }
}
