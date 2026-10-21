// TCP plumbing shared by the scripts `tests/js.rs` drives.

import net from "node:net";
import { Duplex } from "node:stream";

import { fromWebStreams } from "../../js/src/index.js";

/**
 * Split a `host:port` address, such as `[::1]:7761`.
 * @param {string} addr
 */
function parse(addr) {
  const i = addr.lastIndexOf(":");
  return { host: addr.slice(0, i).replace(/^\[(.*)\]$/, "$1"), port: Number(addr.slice(i + 1)) };
}

/**
 * A transport over a socket. wRPC signals the end of each direction by
 * shutting down its write side, so sockets must allow half-open connections.
 * @param {net.Socket} sock
 */
export const transport = (sock) => fromWebStreams(Duplex.toWeb(sock));

/**
 * Connect to `addr`.
 * @param {string} addr
 * @returns {Promise<import("../../js/src/index.js").Transport>}
 */
export function connect(addr) {
  return new Promise((resolve, reject) => {
    const sock = net.connect({ ...parse(addr), allowHalfOpen: true }, () => resolve(transport(sock)));
    sock.once("error", reject);
  });
}

/**
 * Listen on `addr`, calling `f` with a transport for each connection, and
 * print `listening` once ready.
 * @param {string} addr
 * @param {(transport: import("../../js/src/index.js").Transport) => void} f
 */
export function listen(addr, f) {
  const { host, port } = parse(addr);
  net
    .createServer({ allowHalfOpen: true }, (sock) => f(transport(sock)))
    .listen(port, host, () => console.log("listening"));
}
