// The framing multiplexer: the layer that turns a transport's single
// bidirectional byte stream into the indexed sub-streams an invocation needs.
//
// Every wRPC transport (TCP, Unix, QUIC, WebTransport, WebSocket, …) provides
// one bidirectional byte stream per invocation; this module implements the
// mandatory wRPC framing on top of it (see `SPEC.md`, "Framed stream
// specification"). Outgoing data is written as frames `[path][data]`, where
// `path` is a `list<u32>` index identifying the (sub-)stream; incoming frames
// are demultiplexed back to the reader registered for their path.

import { AsyncReader, Chan, Writer } from "./io.js";

/** The wRPC framing protocol version byte. */
export const PROTOCOL = 0x00;

/** @param {number[]} path */
const key = (path) => path.join(",");

/**
 * A byte-stream transport: a duplex of `Uint8Array` chunks.
 * @typedef {object} Transport
 * @property {() => Promise<Uint8Array | null | undefined>} read next inbound chunk, `null` at EOF
 * @property {(bytes: Uint8Array) => Promise<void> | void} write send an outbound chunk
 * @property {() => Promise<void> | void} [closeWrite] half-close the write side
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
 */
export class Mux {
  /** @param {Transport} transport */
  constructor(transport) {
    this.transport = transport;
    /** Reader over the raw inbound byte stream (header, then frames). */
    this.master = new AsyncReader(() => transport.read());
    /** @type {Map<string, Chan<Uint8Array>>} */
    this.channels = new Map();
    /** @type {Map<string, Uint8Array[]>} buffered frames awaiting a reader */
    this.buffered = new Map();
    /** Serializes outbound writes so each frame is emitted atomically. */
    this.writeChain = Promise.resolve();
    this.inboundClosed = false;
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
    this.ingress = this._ingress().catch((err) => {
      this._closeInbound(err);
    });
  }

  async _ingress() {
    const m = this.master;
    for (;;) {
      // Stop cleanly at a frame boundary once the peer closes the stream.
      if (m.remaining().length === 0 && !(await m.more())) break;
      const depth = Number(await m.varU());
      const path = new Array(depth);
      for (let i = 0; i < depth; i++) path[i] = Number(await m.varU());
      const len = Number(await m.varU());
      const data = await m.take(len);
      this._route(path, data);
    }
    this._closeInbound();
  }

  /** @param {number[]} path @param {Uint8Array} data */
  _route(path, data) {
    const k = key(path);
    const chan = this.channels.get(k);
    if (chan) {
      chan.push(data);
      return;
    }
    const pending = this.buffered.get(k);
    if (pending) pending.push(data);
    else this.buffered.set(k, [data]);
  }

  /** @param {Error} [err] */
  _closeInbound(err) {
    this.inboundClosed = true;
    this.inboundError = err;
    for (const chan of this.channels.values()) chan.close(err);
  }

  /**
   * The reader for the sub-stream addressed by `path` (the root channel is
   * `[]`). Any frames that arrived before the call are replayed in order.
   * @param {number[]} path
   * @returns {AsyncReader}
   */
  incoming(path) {
    const k = key(path);
    let chan = this.channels.get(k);
    if (!chan) {
      chan = new Chan();
      const pending = this.buffered.get(k);
      if (pending) {
        for (const data of pending) chan.push(data);
        this.buffered.delete(k);
      }
      if (this.inboundClosed) chan.close(this.inboundError);
      this.channels.set(k, chan);
    }
    const reader = new AsyncReader(async () => {
      const { value, done } = await chan.next();
      return done ? null : value;
    });
    // Allow the value codec to descend into nested sub-streams.
    /** @param {number[]} rel */
    const index = (rel) => this.incoming([...path, ...rel]);
    /** @type {any} */ (reader).index = index;
    return reader;
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
    this.writeChain = this.writeChain.then(() => this.transport.write(bytes));
    return this.writeChain;
  }

  /** Flush pending writes and half-close the write side. @returns {Promise<void>} */
  async closeWrite() {
    await this.writeChain;
    if (this.transport.closeWrite) await this.transport.closeWrite();
  }
}
