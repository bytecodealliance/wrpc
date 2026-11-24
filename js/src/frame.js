// The wRPC framing over a byte stream pair: invocation headers and the frames
// `[path][data]` multiplexing the pending values of an invocation (see
// `SPEC.md`, "Framed stream specification"). Transports provide WHATWG byte
// streams, which their owner ends or tears down.

import { AsyncReader, UNEXPECTED_EOF, Writer, atomic, readVarU32 } from "./io.js";

/** The wRPC framing protocol version byte. */
export const PROTOCOL = 0x01;

const DEFAULT_MAX_DEPTH = 64;
const DEFAULT_MAX_NAME_LENGTH = 1024;
const MAX_FRAME_SIZE = 0xffffffff;
const EMPTY = new Uint8Array(0);

const noop = () => {};

/** Writes invocation headers and frames to a byte stream. */
export class FrameWriter {
  /** @param {WritableStream<Uint8Array>} writable */
  constructor(writable) {
    this.writer = writable.getWriter();
  }

  /** @param {{ instance: string, name: string }} header */
  header({ instance, name }) {
    const w = new Writer();
    w.u8(PROTOCOL);
    w.name(instance);
    w.name(name);
    return this.writer.write(w.finish());
  }

  /**
   * Write `data` as frames on `path`, writing nothing if it is empty.
   * @param {readonly number[]} path @param {Uint8Array} data
   * @returns {Promise<void>}
   */
  write(path, data) {
    let written = Promise.resolve();
    for (let off = 0; off < data.length; off += MAX_FRAME_SIZE) {
      written.catch(noop);
      written = this.#frame(path, data.subarray(off, off + MAX_FRAME_SIZE));
    }
    return written;
  }

  /**
   * Write the frame closing `path`.
   * @param {readonly number[]} path
   */
  close(path) {
    return this.#frame(path, EMPTY);
  }

  /** @param {readonly number[]} path @param {Uint8Array} data */
  #frame(path, data) {
    const w = new Writer();
    w.varU(path.length);
    for (const i of path) w.varU(i);
    w.varU(data.length);
    const written = this.writer.write(w.finish());
    if (!data.length) return written;
    written.catch(noop);
    return this.writer.write(data);
  }
}

/**
 * @param {import("./io.js").Reader} r @param {number} maxDepth
 * @returns {Generator<number, { path: number[], size: number }, void>}
 */
function* frameHeader(r, maxDepth) {
  const n = yield* atomic(r, readVarU32);
  if (n > maxDepth) {
    throw new RangeError(`path length of \`${n}\` exceeds maximum of \`${maxDepth}\``);
  }
  const path = [];
  for (let i = 0; i < n; i++) path.push(yield* atomic(r, readVarU32));
  return { path, size: yield* atomic(r, readVarU32) };
}

/** Reads invocation headers and frames from a byte stream. */
export class FrameReader {
  /**
   * The path of the data last returned by {@link next}.
   * @type {readonly number[]}
   */
  path = [];
  #left = 0;
  #input;

  /**
   * Paths longer than `maxDepth` elements are rejected.
   * @param {ReadableStream<Uint8Array>} readable @param {{ maxDepth?: number }} [options]
   */
  constructor(readable, { maxDepth = DEFAULT_MAX_DEPTH } = {}) {
    this.reader = readable.getReader();
    this.maxDepth = maxDepth;
    this.#input = new AsyncReader(async () => {
      const { value, done } = await this.reader.read();
      return done ? null : value;
    });
  }

  /**
   * The next chunk of the data of a frame on {@link path}, empty for the frame
   * closing it, or `null` once the stream ends between frames. Chunks are
   * views of the chunks read, not copies.
   * @returns {Promise<Uint8Array | null>}
   */
  async next() {
    const input = this.#input;
    if (this.#left === 0) {
      if (!(await input.fill(1))) return null;
      const { path, size } = await input.decode((r) => frameHeader(r, this.maxDepth));
      this.path = path;
      this.#left = size;
      if (size === 0) return EMPTY;
    }
    if (!(await input.fill(1))) throw new RangeError(`${UNEXPECTED_EOF}: frame truncated`);
    const data = input.remaining().subarray(0, this.#left);
    input.consume(data.length);
    this.#left -= data.length;
    return data;
  }

  /**
   * The next invocation header, or `null` once the stream ends before one.
   * Names longer than `maxInstanceLength` and `maxNameLength` bytes, 1 KiB by
   * default, are rejected.
   * @param {{ maxInstanceLength?: number, maxNameLength?: number }} [options]
   * @returns {Promise<{ instance: string, name: string } | null>}
   */
  async header({
    maxInstanceLength = DEFAULT_MAX_NAME_LENGTH,
    maxNameLength = DEFAULT_MAX_NAME_LENGTH,
  } = {}) {
    const input = this.#input;
    if (!(await input.fill(1))) return null;
    const version = await input.u8();
    if (version !== PROTOCOL) throw new RangeError(`unsupported version byte: ${version}`);
    const instance = await input.name(maxInstanceLength);
    const name = await input.name(maxNameLength);
    return { instance, name };
  }
}

/** Frames written to and read from a byte stream pair. */
export class Framed {
  /**
   * @param {WritableStream<Uint8Array>} writable @param {ReadableStream<Uint8Array>} readable
   * @param {{ maxDepth?: number }} [options]
   */
  constructor(writable, readable, options) {
    this.tx = new FrameWriter(writable);
    this.rx = new FrameReader(readable, options);
  }
}

/** Accepts the invocations sent over a byte stream pair one after another. */
export class Listener {
  #options;

  /**
   * @param {WritableStream<Uint8Array>} writable @param {ReadableStream<Uint8Array>} readable
   * @param {{ maxDepth?: number, maxInstanceLength?: number, maxNameLength?: number }} [options]
   */
  constructor(writable, readable, options = {}) {
    this.io = new Framed(writable, readable, options);
    this.#options = options;
  }

  /**
   * The header of the next invocation, once the previous one was served, along
   * with the frames to serve it over, or `null` once the stream ends before one.
   * Once serving an invocation failed, the byte streams must be torn down.
   * @returns {Promise<[{ instance: string, name: string }, Framed] | null>}
   */
  async accept() {
    const header = await this.io.rx.header(this.#options);
    return header && [header, this.io];
  }
}
