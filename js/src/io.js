// Byte-stream reader and writer with the integer and string codecs the wRPC
// wire format is built from: LEB128 integers and `core:name` length-prefixed
// UTF-8 strings. See the wRPC `SPEC.md` and the component model value
// definition encoding.

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** The message thrown by readers when the input is exhausted mid-value. */
export const UNEXPECTED_EOF = "unexpected end of input";

/** @param {string} sign @param {number} bits */
const overflow = (sign, bits) => new RangeError(`LEB128 integer overflows ${sign} ${bits}-bit range`);

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
    if (typeof value === "number" && value >>> 0 === value) {
      for (; value >= 0x80; value >>>= 7) this.u8((value & 0x7f) | 0x80);
      this.u8(value);
      return;
    }
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
    if (typeof value === "number" && (value | 0) === value) {
      for (;;) {
        const byte = value & 0x7f;
        value >>= 7;
        if ((value === 0 && !(byte & 0x40)) || (value === -1 && byte & 0x40)) {
          this.u8(byte);
          return;
        }
        this.u8(byte | 0x80);
      }
    }
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
    /** The buffer length the last end-of-input underflow needed. */
    this.need = 0;
  }

  /** @returns {boolean} whether the cursor is at the end of the buffer */
  get done() {
    return this.pos >= this.bytes.length;
  }

  /** Read a single byte. @returns {number} */
  u8() {
    this.#ensure(1);
    return this.bytes[this.pos++];
  }

  /** Read the next byte without consuming it. @returns {number} */
  peek() {
    this.#ensure(1);
    return this.bytes[this.pos];
  }

  /** Read `n` raw bytes. @param {number} n @returns {Uint8Array} */
  take(n) {
    this.#ensure(n);
    const slice = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return slice;
  }

  /** @param {number} n */
  #ensure(n) {
    if (this.pos + n > this.bytes.length) {
      this.need = this.pos + n;
      throw new RangeError(UNEXPECTED_EOF);
    }
  }

  /**
   * Read an unsigned LEB128 integer of at most `bits` bits.
   * @param {number} [bits] @returns {bigint}
   */
  varU(bits = 64) {
    return this.#leb128(bits, false);
  }

  /**
   * Read a signed LEB128 integer of at most `bits` bits.
   * @param {number} [bits] @returns {bigint}
   */
  varS(bits = 64) {
    return this.#leb128(bits, true);
  }

  /** @param {number} bits @param {boolean} signed @returns {bigint} */
  #leb128(bits, signed) {
    if (bits <= 32) return BigInt(readVarInt(this, bits, signed));
    const sign = signed ? "signed" : "unsigned";
    let result = 0n;
    let shift = 0;
    let byte;
    do {
      if (shift >= bits) throw overflow(sign, bits);
      byte = this.u8();
      result |= BigInt(byte & 0x7f) << BigInt(shift);
      shift += 7;
    } while (byte & 0x80);
    if (!signed) {
      if (result >> BigInt(bits)) throw overflow(sign, bits);
      return result;
    }
    if (byte & 0x40) result |= -1n << BigInt(shift); // sign-extend
    if (BigInt.asIntN(bits, result) !== result) throw overflow(sign, bits);
    return result;
  }

  /** Read a `core:name`. @returns {string} */
  name() {
    return textDecoder.decode(this.take(readVarU32(this)));
  }
}

/**
 * Read a LEB128 integer of at most 32 bits as a number.
 * @param {Reader} r @param {number} bits @param {boolean} signed @returns {number}
 */
export function readVarInt(r, bits, signed) {
  let result = 0;
  let shift = 0;
  let byte;
  do {
    if (shift >= bits) throw overflow(signed ? "signed" : "unsigned", bits);
    byte = r.u8();
    result += (byte & 0x7f) * 2 ** shift;
    shift += 7;
  } while (byte & 0x80);
  if (signed && byte & 0x40) result -= 2 ** shift; // sign-extend
  const [min, max] = signed ? [-(2 ** (bits - 1)), 2 ** (bits - 1)] : [0, 2 ** bits];
  if (result < min || result >= max) throw overflow(signed ? "signed" : "unsigned", bits);
  return result;
}

/** Read an unsigned LEB128 `u32` as a number. @param {Reader} r */
export const readVarU32 = (r) => readVarInt(r, 32, false);

/** @param {unknown} err @returns {boolean} whether `err` is an end-of-input underflow */
export function isUnexpectedEof(err) {
  return err instanceof RangeError && err.message.startsWith(UNEXPECTED_EOF);
}

/**
 * Run the synchronous read `f` as one step of a resumable decoder: if the
 * input runs out partway, rewind `r` and yield the number of bytes the step
 * needs from there, then retry it once they are buffered.
 * @template T
 * @param {Reader} r
 * @param {(r: Reader) => T} f
 * @returns {Generator<number, T, void>}
 */
export function* atomic(r, f) {
  for (;;) {
    const pos = r.pos;
    try {
      return f(r);
    } catch (err) {
      if (!isUnexpectedEof(err)) throw err;
      r.pos = pos;
      yield r.need - pos;
    }
  }
}

/**
 * An async FIFO queue. Producers `push` values, or `send` them to wait while
 * `capacity` values are buffered, and `close` the queue; consumers
 * `await next()` or iterate it with `for await`, and `cancel` it to stop
 * consuming. Once closed, pending and subsequent `next()` calls resolve to
 * `{ done: true }`, or reject with the error passed to `close` if the channel
 * was failed.
 *
 * @template T
 */
export class Chan {
  /** @param {number} [capacity] the number of values `send` buffers */
  constructor(capacity = Infinity) {
    this.capacity = capacity;
    /** @type {T[]} */
    this.queue = [];
    /** @type {{ resolve: (r: { value: T | undefined, done: boolean }) => void, reject: (e: unknown) => void }[]} */
    this.waiters = [];
    /** @type {(() => void)[]} */
    this.senders = [];
    this.closed = false;
    this.cancelled = false;
    /** The number of values handed to the consumer. */
    this.taken = 0;
    /** @type {unknown} */
    this.error = undefined;
  }

  /** @type {(() => void)[]} */
  #cancelHooks = [];

  /**
   * Call `f` once the consumer cancels the channel, or now if it already has.
   * Hooks are dropped once the consumer has received the end of the channel:
   * cancelling then drops nothing.
   * @param {() => void} f
   */
  onCancel(f) {
    if (this.cancelled) f();
    else this.#cancelHooks.push(f);
  }

  /** Push a value. Ignored once the channel is closed. @param {T} value */
  push(value) {
    if (this.closed) return;
    const w = this.waiters.shift();
    if (!w) {
      this.queue.push(value);
      return;
    }
    this.taken++;
    w.resolve({ value, done: false });
  }

  /**
   * Push a value once fewer than `capacity` values are buffered.
   * @param {T} value
   * @returns {Promise<boolean>} `false` if the channel was closed instead
   */
  async send(value) {
    while (!this.closed && this.queue.length >= this.capacity) {
      await new Promise((resolve) => this.senders.push(() => resolve(undefined)));
    }
    if (this.closed) return false;
    this.push(value);
    return true;
  }

  /**
   * Stop consuming: drop the buffered values, close (failed with `err`, if
   * given and it has not already failed), and call the `onCancel` hooks.
   * @param {unknown} [err]
   */
  cancel(err) {
    if (this.cancelled) return;
    this.cancelled = true;
    this.queue.length = 0;
    this.close(err);
    this.error ??= err;
    for (const f of this.#cancelHooks.splice(0)) f();
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
    if (this.waiters.length && !this.cancelled) this.#cancelHooks.length = 0;
    let w;
    while ((w = this.waiters.shift())) {
      if (err !== undefined) w.reject(err);
      else w.resolve({ value: undefined, done: true });
    }
    for (const wake of this.senders.splice(0)) wake();
  }

  /** @returns {Promise<{ value: T | undefined, done: boolean }>} */
  next() {
    if (this.queue.length) {
      const value = /** @type {T} */ (this.queue.shift());
      this.taken++;
      this.senders.shift()?.();
      return Promise.resolve({ value, done: false });
    }
    if (this.closed) {
      this.#cancelHooks.length = 0;
      return this.error !== undefined
        ? Promise.reject(this.error)
        : Promise.resolve({ value: undefined, done: true });
    }
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  /** Iterating it to the end consumes it; leaving the loop early cancels it. */
  [Symbol.asyncIterator]() {
    return {
      next: () => this.next(),
      return: async () => {
        this.cancel();
        return /** @type {const} */ ({ value: undefined, done: true });
      },
    };
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
   * Pull chunks until at least `n` unconsumed bytes are buffered, joining them
   * with a single copy.
   * @param {number} n
   * @returns {Promise<boolean>} `false` if the input ended first
   */
  async fill(n) {
    let have = this.buf.length - this.pos;
    if (have >= n) return true;
    const chunks = have > 0 ? [this.remaining()] : [];
    while (have < n && !this.eof) {
      const chunk = await this.pull();
      if (chunk === null || chunk === undefined) {
        this.eof = true;
      } else if (chunk.length > 0) {
        chunks.push(chunk);
        have += chunk.length;
      }
    }
    if (chunks.length === 1) {
      this.buf = chunks[0];
    } else {
      this.buf = new Uint8Array(have);
      let off = 0;
      for (const chunk of chunks) {
        this.buf.set(chunk, off);
        off += chunk.length;
      }
    }
    this.pos = 0;
    return have >= n;
  }

  /** Read a single byte, awaiting input if needed. @returns {Promise<number>} */
  async u8() {
    if (!(await this.fill(1))) throw new RangeError(UNEXPECTED_EOF);
    return this.buf[this.pos++];
  }

  /** Read `n` bytes, awaiting input if needed. @param {number} n @returns {Promise<Uint8Array>} */
  async take(n) {
    if (!(await this.fill(n))) throw new RangeError(UNEXPECTED_EOF);
    const slice = this.buf.slice(this.pos, this.pos + n);
    this.pos += n;
    return slice;
  }

  /**
   * Run the resumable decoder `decode` over the buffered bytes: each time it
   * yields the number of bytes it needs next, consume what it has read so far
   * and pull more input.
   * @template T
   * @param {(r: Reader) => Generator<number, T, void>} decode
   * @returns {Promise<T>}
   */
  async decode(decode) {
    const r = new Reader(this.remaining());
    const steps = decode(r);
    for (let step = steps.next(); ; step = steps.next()) {
      this.consume(r.pos);
      if (step.done) return step.value;
      if (!(await this.fill(step.value))) throw new RangeError(UNEXPECTED_EOF);
      r.bytes = this.remaining();
      r.pos = 0;
    }
  }

  /**
   * Read an unsigned LEB128 integer of at most `bits` bits.
   * @param {number} [bits] @returns {Promise<bigint>}
   */
  varU(bits = 64) {
    return this.decode((r) => atomic(r, (r) => r.varU(bits)));
  }

  /** Read a `core:name`. @returns {Promise<string>} */
  name() {
    return this.decode((r) => atomic(r, (r) => r.name()));
  }
}

export { textEncoder, textDecoder };
