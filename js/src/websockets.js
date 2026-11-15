// The WebSocket transport: one invocation at a time per connection, whose
// binary messages carry the byte stream in both directions.

import { Chan } from "./io.js";

/**
 * @typedef {import("./mux.js").Transport} Transport
 */

/** The status code closing a connection once its invocation failed. */
const FAILED = 4000;

/** The maximum length of a message sent. */
const MAX_MESSAGE_LEN = 1 << 20;

/** The maximum number of bytes received and not read, or buffered to be sent. */
const MAX_BUFFERED = 16 << 20;

/**
 * Adapt a built-in `WebSocket` to a {@link Transport} for invocations one at a
 * time, sent as binary messages of at most 1 MiB. Use one transport for all invocations on a
 * connection, since each one receives every message. Reading ends once the peer closes the
 * connection with status code `1000` or ends it without the closing handshake,
 * and fails on a text message or any other close. A WebSocket has no
 * half-close, so `closeWrite` does nothing: close the connection with status
 * code `1000` once the last invocation is finished. Aborting closes it with status code `4000`. The
 * built-in `WebSocket` cannot apply backpressure, so the connection is closed with status code
 * `4000` once more than 16 MiB received are not read, and writes resolve once less than 16 MiB
 * are buffered.
 * @param {WebSocket} ws
 * @returns {Transport}
 */
export function fromWebSocket(ws) {
  ws.binaryType = "arraybuffer";
  /** @type {Chan<Uint8Array>} */
  const inbound = new Chan();
  let buffered = 0;
  if (ws.readyState === ws.CLOSED) inbound.close(new Error("WebSocket connection is closed"));
  ws.addEventListener("message", ({ data }) => {
    if (typeof data === "string") {
      inbound.close(new Error("unexpected WebSocket text message"));
    } else if ((buffered += data.byteLength) > MAX_BUFFERED) {
      inbound.cancel(new Error("WebSocket receive buffer of 16 MiB exceeded"));
      ws.close(FAILED);
    } else {
      inbound.push(new Uint8Array(data));
    }
  });
  ws.addEventListener("close", ({ code, reason }) => {
    inbound.close(
      code === 1000 || code === 1006
        ? undefined
        : new Error(`WebSocket connection closed with code ${code}${reason ? `: ${reason}` : ""}`),
    );
  });
  const opened = new Promise((resolve) => {
    if (ws.readyState !== ws.CONNECTING) return resolve(undefined);
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("close", resolve, { once: true });
  });
  return {
    read: async () => {
      const { value } = await inbound.next();
      if (!value) return null;
      buffered -= value.length;
      return value;
    },
    write: async (bytes) => {
      await opened;
      for (let i = 0; i < bytes.length; i += MAX_MESSAGE_LEN) {
        if (ws.readyState !== ws.OPEN) throw new Error("WebSocket connection is not open");
        ws.send(bytes.subarray(i, i + MAX_MESSAGE_LEN));
        while (ws.bufferedAmount >= MAX_BUFFERED) {
          await new Promise((resolve) => setTimeout(resolve, 10));
          if (ws.readyState !== ws.OPEN) throw new Error("WebSocket connection is not open");
        }
      }
    },
    closeWrite: () => {},
    abort: () => ws.close(FAILED),
  };
}
