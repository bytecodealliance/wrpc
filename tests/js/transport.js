// TCP and HTTP plumbing shared by the scripts `tests/js.rs` drives. An
// address of the form `http://host:port` selects HTTP, any other TCP.

import http2 from "node:http2";
import net from "node:net";
import { Duplex, Readable, pipeline } from "node:stream";

import {
  Framed,
  Listener,
  decoder,
  encoder,
  fromFetch,
  fromRequest,
  fromWebStreams,
  receive,
  t,
  transmit,
} from "../../js/src/index.js";

/**
 * @typedef {import("../../js/src/index.js").InputValue} InputValue
 * @typedef {import("../../js/src/index.js").Transport} Transport
 * @typedef {import("../../js/src/index.js").Type} Type
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
        res.writeHead(response.status, Object.fromEntries(response.headers));
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
 * Connect to the TCP `addr`, returning the frames over the connection.
 * @param {string} addr
 * @returns {Promise<Framed>}
 */
export function connectFramed(addr) {
  return new Promise((resolve, reject) => {
    const sock = net.connect({ ...parse(addr), allowHalfOpen: true }, () =>
      resolve(new Framed(...streams(sock))),
    );
    sock.once("error", reject);
  });
}

/**
 * Listen on the TCP `addr`, calling `f` with a listener of the invocations
 * on each connection and the connection, and print `listening` once ready.
 * @param {string} addr
 * @param {(lis: Listener, sock: net.Socket) => void} f
 */
export function listenFramed(addr, f) {
  const { host, port } = parse(addr);
  net
    .createServer({ allowHalfOpen: true }, (sock) => f(new Listener(...streams(sock)), sock))
    .listen(port, host, () => console.log("listening"));
}

/**
 * Invoke `instance#name` over `io` using the codec tree.
 * @param {Framed} io @param {string} instance @param {string} name
 * @param {Type[]} params @param {InputValue[]} values @param {Type[]} results
 * @returns {Promise<[any[], Promise<unknown>]>}
 */
export async function invokeFramed(io, instance, name, params, values, results) {
  await io.tx.header({ instance, name });
  const tx = transmit(encoder(t.tuple(...params)), io.tx, values);
  const [out, rx] = await Promise.race([
    receive(decoder(t.tuple(...results)), io.rx),
    tx.then(() => new Promise(() => {})),
  ]);
  return [/** @type {any[]} */ (out), Promise.all([tx, rx])];
}

/**
 * Serve an invocation over `io` with `f` using the codec tree.
 * @param {Framed} io @param {Type[]} params @param {Type[]} results
 * @param {(...params: any[]) => InputValue[] | Promise<InputValue[]>} f
 */
export async function serveFramed(io, params, results, f) {
  const [values, rx] = await receive(decoder(t.tuple(...params)), io.rx);
  const tx = (async () =>
    transmit(encoder(t.tuple(...results)), io.tx, await f(.../** @type {any[]} */ (values))))();
  await Promise.all([rx, tx]);
}
