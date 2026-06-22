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

export { textEncoder, textDecoder };
