// TCP and HTTP plumbing shared by the scripts `tests/js.rs` drives. An
// address of the form `http://host:port` selects HTTP, any other TCP.

import http2 from "node:http2";
import net from "node:net";
import { Duplex, Readable, pipeline } from "node:stream";

import { Framed, Listener, fromFetch } from "../../js/src/index.js";

/**
 * Split a `host:port` address, such as `[::1]:7761`.
 * @param {string} addr
 */
function parse(addr) {
  const i = addr.lastIndexOf(":");
  return { host: addr.slice(0, i).replace(/^\[(.*)\]$/, "$1"), port: Number(addr.slice(i + 1)) };
}

/**
 * The byte streams of `sock`, which must allow half-open connections.
 * @param {net.Socket} sock
 */
function streams(sock) {
  sock.setNoDelay(true);
  const { readable, writable } = Duplex.toWeb(sock);
  return /** @type {const} */ ([writable, readable]);
}

/**
 * Connect to `addr`, returning the frames over the TCP connection, or a
 * client sending a request per invocation over HTTP.
 * @param {string} addr
 * @returns {Promise<import("../../js/src/index.js").Invoke>}
 */
export function connect(addr) {
  if (addr.startsWith("http:")) return Promise.resolve(fromFetch(addr));
  return new Promise((resolve, reject) => {
    const sock = net.connect({ ...parse(addr), allowHalfOpen: true }, () =>
      resolve(new Framed(...streams(sock))),
    );
    sock.once("error", reject);
  });
}

/**
 * Listen on `addr`, calling `f` with a listener of the invocations on each
 * TCP connection and the connection, or `request` with each HTTP/2 request
 * without TLS, responding with the response it resolves to, and print
 * `listening` once ready.
 * @param {string} addr
 * @param {(lis: Listener, sock: net.Socket) => void} f
 * @param {(req: Request) => Promise<Response>} request
 */
export function listen(addr, f, request) {
  if (addr.startsWith("http:")) {
    const { hostname, port } = new URL(addr);
    http2
      .createServer(async (req, res) => {
        const init = { method: req.method, body: Readable.toWeb(req), duplex: "half" };
        try {
          const response = await request(
            new Request(new URL(req.url, addr), /** @type {RequestInit} */ (init)),
          );
          res.writeHead(response.status, Object.fromEntries(response.headers));
          pipeline(Readable.fromWeb(/** @type {any} */ (response.body)), res, () => {});
        } catch (err) {
          console.error("failed to serve:", err);
          res.stream.close(http2.constants.NGHTTP2_INTERNAL_ERROR);
        }
      })
      .listen(Number(port), hostname.replace(/^\[(.*)\]$/, "$1"), () => console.log("listening"));
    return;
  }
  const { host, port } = parse(addr);
  net
    .createServer({ allowHalfOpen: true }, (sock) => f(new Listener(...streams(sock)), sock))
    .listen(port, host, () => console.log("listening"));
}
