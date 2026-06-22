// Byte-stream reader and writer with the integer and string codecs the wRPC
// wire format is built from: LEB128 integers and `core:name` length-prefixed
// UTF-8 strings. See the wRPC `SPEC.md` and the component model value
// definition encoding.

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/** A growable little-endian byte writer. */
export class Writer {
  constructor() {
    this.buf = new Uint8Array(64);
    this.len = 0;
  }

  /** @param {number} extra number of additional bytes that must fit */
  #reserve(extra) {
    const need = this.len + extra;
    if (need <= this.buf.length) return;
    let cap = this.buf.length * 2;
    while (cap < need) cap *= 2;
    const grown = new Uint8Array(cap);
    grown.set(this.buf.subarray(0, this.len));
    this.buf = grown;
  }

  /** Write a single byte. @param {number} b */
  u8(b) {
    this.#reserve(1);
    this.buf[this.len++] = b & 0xff;
  }

  /** Write raw bytes. @param {Uint8Array | number[]} bytes */
  bytes(bytes) {
    this.#reserve(bytes.length);
    this.buf.set(bytes, this.len);
    this.len += bytes.length;
  }

  /**
   * Write an unsigned LEB128 integer.
   * @param {number | bigint} value non-negative integer
   */
  varU(value) {
    let v = BigInt(value);
    if (v < 0n) throw new RangeError(`expected a non-negative integer, got ${v}`);
    do {
      let byte = Number(v & 0x7fn);
      v >>= 7n;
      if (v !== 0n) byte |= 0x80;
      this.u8(byte);
    } while (v !== 0n);
  }

  /**
   * Write a signed LEB128 integer.
   * @param {number | bigint} value
   */
  varS(value) {
    let v = BigInt(value);
    for (;;) {
      const byte = Number(v & 0x7fn);
      v >>= 7n; // BigInt `>>` is an arithmetic (sign-extending) shift
      const signBit = byte & 0x40;
      if ((v === 0n && !signBit) || (v === -1n && signBit)) {
        this.u8(byte);
        return;
      }
      this.u8(byte | 0x80);
    }
  }

  /**
   * Write a `core:name`: an unsigned LEB128 length prefix followed by the
   * UTF-8 bytes.
   * @param {string} str
   */
  name(str) {
    const utf8 = textEncoder.encode(str);
    this.varU(utf8.length);
    this.bytes(utf8);
  }

  /** @returns {Uint8Array} a copy of the written bytes */
  finish() {
    return this.buf.slice(0, this.len);
  }
}

/** A cursor over a byte buffer with the matching decoders. */
export class Reader {
  /** @param {Uint8Array} bytes */
  constructor(bytes) {
    this.bytes = bytes;
    this.pos = 0;
  }

  /** @returns {boolean} whether the cursor is at the end of the buffer */
  get done() {
    return this.pos >= this.bytes.length;
  }

  /** Read a single byte. @returns {number} */
  u8() {
    if (this.pos >= this.bytes.length) throw new RangeError("unexpected end of input");
    return this.bytes[this.pos++];
  }

  /** Read the next byte without consuming it. @returns {number} */
  peek() {
    if (this.pos >= this.bytes.length) throw new RangeError("unexpected end of input");
    return this.bytes[this.pos];
  }

  /** Read `n` raw bytes. @param {number} n @returns {Uint8Array} */
  take(n) {
    if (this.pos + n > this.bytes.length) throw new RangeError("unexpected end of input");
    const slice = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return slice;
  }

  /** Read an unsigned LEB128 integer. @returns {bigint} */
  varU() {
    let result = 0n;
    let shift = 0n;
    let byte;
    do {
      byte = this.u8();
      result |= BigInt(byte & 0x7f) << shift;
      shift += 7n;
    } while (byte & 0x80);
    return result;
  }

  /** Read a signed LEB128 integer. @returns {bigint} */
  varS() {
    let result = 0n;
    let shift = 0n;
    let byte;
    do {
      byte = this.u8();
      result |= BigInt(byte & 0x7f) << shift;
      shift += 7n;
    } while (byte & 0x80);
    if (byte & 0x40) result |= -1n << shift; // sign-extend
    return result;
  }

  /** Read a `core:name`. @returns {string} */
  name() {
    const len = Number(this.varU());
    return textDecoder.decode(this.take(len));
  }
}

/** The message thrown by readers when the input is exhausted mid-value. */
export const UNEXPECTED_EOF = "unexpected end of input";

/** @param {unknown} err @returns {boolean} whether `err` is an end-of-input underflow */
export function isUnexpectedEof(err) {
  return err instanceof RangeError && err.message.startsWith(UNEXPECTED_EOF);
}

/**
 * An unbounded async FIFO queue. Producers `push` values (or `close` the
 * queue); consumers `await next()` or iterate it with `for await`. Once
 * closed, pending and subsequent `next()` calls resolve to `{ done: true }`,
 * or reject with the error passed to `close` if the channel was failed.
 *
 * @template T
 */
export class Chan {
  constructor() {
    /** @type {T[]} */
    this.queue = [];
    /** @type {{ resolve: (r: { value: T | undefined, done: boolean }) => void, reject: (e: unknown) => void }[]} */
    this.waiters = [];
    this.closed = false;
    /** @type {unknown} */
    this.error = undefined;
  }

  /** Push a value. Ignored once the channel is closed. @param {T} value */
  push(value) {
    if (this.closed) return;
    const w = this.waiters.shift();
    if (w) w.resolve({ value, done: false });
    else this.queue.push(value);
  }

  /**
   * Close the channel, releasing every pending and future `next()`. When `err`
   * is given the channel is failed: once buffered values are drained, `next()`
   * rejects with `err` instead of signalling a clean end.
   * @param {unknown} [err]
   */
  close(err) {
    if (this.closed) return;
    this.closed = true;
    this.error = err;
    let w;
    while ((w = this.waiters.shift())) {
      if (err !== undefined) w.reject(err);
      else w.resolve({ value: undefined, done: true });
    }
  }

  /** @returns {Promise<{ value: T | undefined, done: boolean }>} */
  next() {
    if (this.queue.length) return Promise.resolve({ value: this.queue.shift(), done: false });
    if (this.closed) {
      return this.error !== undefined
        ? Promise.reject(this.error)
        : Promise.resolve({ value: undefined, done: true });
    }
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  [Symbol.asyncIterator]() {
    return { next: () => this.next() };
  }
}

/**
 * A pull-based async byte reader with the same integer/`core:name` decoders as
 * {@link Reader}, but able to await more input across chunk boundaries. It is
 * driven by a `pull` function returning the next chunk (or `null`/`undefined`
 * at end of input), and buffers any leftover bytes between reads.
 */
export class AsyncReader {
  /** @param {() => Promise<Uint8Array | null | undefined>} pull */
  constructor(pull) {
    this.pull = pull;
    /** @type {Uint8Array} */
    this.buf = new Uint8Array(0);
    this.pos = 0;
    this.eof = false;
  }

  /** The bytes buffered but not yet consumed. @returns {Uint8Array} */
  remaining() {
    return this.buf.subarray(this.pos);
  }

  /** Mark `n` buffered bytes as consumed. @param {number} n */
  consume(n) {
    this.pos += n;
  }

  /**
   * Pull one more chunk into the buffer.
   * @returns {Promise<boolean>} `false` once the input is exhausted
   */
  async more() {
    if (this.eof) return false;
    const chunk = await this.pull();
    if (chunk === null || chunk === undefined) {
      this.eof = true;
      return false;
    }
    if (chunk.length === 0) return this.more();
    const rem = this.remaining();
    if (rem.length === 0) {
      this.buf = chunk;
      this.pos = 0;
    } else {
      const merged = new Uint8Array(rem.length + chunk.length);
      merged.set(rem, 0);
      merged.set(chunk, rem.length);
      this.buf = merged;
      this.pos = 0;
    }
    return true;
  }

  /** Read a single byte, awaiting input if needed. @returns {Promise<number>} */
  async u8() {
    while (this.pos >= this.buf.length) {
      if (!(await this.more())) throw new RangeError(UNEXPECTED_EOF);
    }
    return this.buf[this.pos++];
  }

  /** Read `n` bytes, awaiting input if needed. @param {number} n @returns {Promise<Uint8Array>} */
  async take(n) {
    while (this.buf.length - this.pos < n) {
      if (!(await this.more())) throw new RangeError(UNEXPECTED_EOF);
    }
    const slice = this.buf.slice(this.pos, this.pos + n);
    this.pos += n;
    return slice;
  }

  /** Read an unsigned LEB128 integer. @returns {Promise<bigint>} */
  async varU() {
    let result = 0n;
    let shift = 0n;
    let byte;
    do {
      byte = await this.u8();
      result |= BigInt(byte & 0x7f) << shift;
      shift += 7n;
    } while (byte & 0x80);
    return result;
  }

  /** Read a `core:name`. @returns {Promise<string>} */
  async name() {
    const len = Number(await this.varU());
    return textDecoder.decode(await this.take(len));
  }
}

export { textEncoder, textDecoder };
