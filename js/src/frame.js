// The default wRPC framing for non-multiplexed byte-stream transports (the same
// framing `wrpc-transport`'s `frame` module implements, used by TCP, Unix,
// WebSocket and WebTransport).
//
// A client writes, on the connection's root stream:
//   - the framing protocol byte `0x00`,
//   - the `core:name`-encoded instance and function names,
//   - the root frame `[path-len=0][data-len][params]`, whose data is the
//     concatenated encoding of the parameters.
//
// The server replies with the concatenated encoding of the results in its own
// root frame `[path-len=0][data-len][results]`.
//
// This module covers the synchronous root channel only — the indexed
// sub-streams used for async params/results (`stream`, `future`) are not
// handled here.

import { Reader, Writer } from "./io.js";
import { encodeValue, decodeValue } from "./value.js";

/** @typedef {import("./index.d.ts").Type} Type */

/** The wRPC framing protocol version byte. */
export const PROTOCOL = 0x00;

/**
 * Build the bytes a client writes to invoke `func` on `instance`: the protocol
 * byte, the instance and function names, and the root parameter frame.
 * @param {string} instance
 * @param {string} func
 * @param {Type[]} paramTypes
 * @param {any[]} args
 * @returns {Uint8Array}
 */
export function encodeInvocation(instance, func, paramTypes, args) {
  if (args.length !== paramTypes.length) {
    throw new RangeError(`expected ${paramTypes.length} argument(s), got ${args.length}`);
  }
  const params = new Writer();
  paramTypes.forEach((ty, i) => encodeValue(params, ty, args[i]));
  const paramBytes = params.finish();

  const w = new Writer();
  w.u8(PROTOCOL);
  w.name(instance);
  w.name(func);
  w.u8(0); // root frame path length
  w.varU(paramBytes.length); // root frame data length
  w.bytes(paramBytes);
  return w.finish();
}

/**
 * Decode the server's root result frame `[path-len=0][data-len][data]` into the
 * result values. A function with no results writes no frame, so an empty input
 * is expected (and returns `[]`) only when no results are expected. If results
 * are expected but the input is empty, the peer closed the stream without
 * responding (e.g. the invocation failed server-side) and an error is thrown.
 * @param {Uint8Array} bytes
 * @param {Type[]} resultTypes
 * @returns {any[]}
 */
export function decodeResults(bytes, resultTypes) {
  if (resultTypes.length === 0) return [];
  if (bytes.length === 0) {
    throw new Error(
      "peer closed the stream without sending a result frame " +
        "(the invocation likely failed on the other end)",
    );
  }
  const frame = new Reader(bytes);
  const pathLen = Number(frame.varU());
  if (pathLen !== 0) throw new Error(`unexpected result frame path length ${pathLen}`);
  const dataLen = Number(frame.varU());
  const data = new Reader(frame.take(dataLen));
  return resultTypes.map((ty) => decodeValue(data, ty));
}

/**
 * Concatenate a list of byte chunks into a single `Uint8Array`.
 * @param {Uint8Array[]} chunks
 */
export function concatBytes(chunks) {
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}
