// TCP and HTTP plumbing shared by the scripts `tests/js.rs` drives. An
// address of the form `http://host:port` selects HTTP, any other TCP.

import http2 from "node:http2";
import net from "node:net";
import { Duplex, Readable, pipeline } from "node:stream";

import { fromFetch, fromRequest, fromWebStreams } from "../../js/src/index.js";

/**
 * @typedef {import("../../js/src/index.js").Transport} Transport
 */

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
 * shutting down its write side, so sockets must allow half-open connections,
 * and a failed invocation resets the connection.
 * @param {net.Socket} sock
 * @returns {Transport}
 */
export const transport = (sock) => ({
  ...fromWebStreams(Duplex.toWeb(sock)),
  abort: () => void sock.resetAndDestroy(),
});

/**
 * Connect to `addr`.
 * @param {string} addr
 * @returns {Promise<Transport>}
 */
export function connect(addr) {
  if (addr.startsWith("http:")) return Promise.resolve(fromFetch(addr));
  return new Promise((resolve, reject) => {
    const sock = net.connect({ ...parse(addr), allowHalfOpen: true }, () =>
      resolve(transport(sock)),
    );
    sock.once("error", reject);
  });
}

/**
 * Listen on `addr`, calling `f` with a transport for each connection, or
 * each HTTP/2 request without TLS, and print `listening` once ready.
 * @param {string} addr
 * @param {(transport: Transport) => void} f
 */
export function listen(addr, f) {
  if (addr.startsWith("http:")) {
    const { hostname, port } = new URL(addr);
    http2
      .createServer((req, res) => {
        const init = { method: req.method, body: Readable.toWeb(req), duplex: "half" };
        const { transport, response } = fromRequest(
          new Request(new URL(req.url, addr), /** @type {RequestInit} */ (init)),
        );
        f(transport);
        res.writeHead(response.status);
        pipeline(Readable.fromWeb(/** @type {any} */ (response.body)), res, () => {});
      })
      .listen(Number(port), hostname.replace(/^\[(.*)\]$/, "$1"), () => console.log("listening"));
    return;
  }
  const { host, port } = parse(addr);
  net
    .createServer({ allowHalfOpen: true }, (sock) => f(transport(sock)))
    .listen(port, host, () => console.log("listening"));
}
