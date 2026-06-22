import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { Chan } from "../src/io.js";
import { Mux } from "../src/mux.js";
import { invoke, accept } from "../src/session.js";

/**
 * A pair of in-memory transports wired back to back, modelling a single
 * bidirectional byte stream per invocation.
 */
function duplexPair() {
  const a = new Chan();
  const b = new Chan();
  const end = (chan) => ({
    read: async () => {
      const { value, done } = await chan.read.next();
      return done ? null : value;
    },
    write: (bytes) => {
      chan.write.push(bytes);
    },
    closeWrite: () => {
      chan.write.close();
    },
  });
  return [end({ read: a, write: b }), end({ read: b, write: a })];
}

/** Drain an async iterable of chunks into a flat array. */
async function collect(stream) {
  const out = [];
  for await (const chunk of stream) out.push(...chunk);
  return out;
}

const STREAMS = "wrpc-examples:streams/handler";
const req = t.record({ numbers: t.stream(t.u64), bytes: t.stream(t.u8) });
const echoResult = t.tuple(t.stream(t.u64), t.stream(t.u8));

test("stream parameters and results echo end to end", async () => {
  const [client, server] = duplexPair();

  // A server that echoes the request's two streams straight back.
  const serving = (async () => {
    const inv = await accept(server);
    assert.equal(inv.instance, STREAMS);
    assert.equal(inv.func, "echo");
    const { params, done: rx } = await inv.receiveParams([req]);
    const { numbers, bytes } = params[0];
    await Promise.all([rx, inv.sendResults([echoResult], [[numbers, bytes]])]);
  })();

  const { results, done } = await invoke(
    client,
    STREAMS,
    "echo",
    [req],
    [{ numbers: [[1n, 2n, 3n], [4n, 5n]], bytes: new Uint8Array([10, 20, 30, 40]) }],
    [echoResult],
  );

  const [numbers, bytes] = results[0];
  assert.deepEqual(await collect(numbers), [1n, 2n, 3n, 4n, 5n]);
  assert.deepEqual(await collect(bytes), [10, 20, 30, 40]);

  await Promise.all([done, serving]);
});

test("empty interior stream chunks do not truncate the stream", async () => {
  const ty = t.tuple(t.stream(t.u64));
  const [client, server] = duplexPair();
  const serving = (async () => {
    const inv = await accept(server);
    const { params, done: rx } = await inv.receiveParams([ty]);
    await Promise.all([rx, inv.sendResults([ty], [params[0]])]);
  })();

  // The empty middle chunk must not be read as the stream terminator.
  const { results, done } = await invoke(
    client,
    STREAMS,
    "echo",
    [ty],
    [[[[1n, 2n], [], [3n, 4n]]]],
    [ty],
  );
  assert.deepEqual(await collect(results[0][0]), [1n, 2n, 3n, 4n]);
  await Promise.all([done, serving]);
});

test("an inbound transport failure surfaces to a sub-stream reader", async () => {
  /** @type {(e: Error) => void} */
  let fail = () => {};
  const transport = {
    read: () => new Promise((_, reject) => (fail = reject)),
    write: () => {},
  };
  const mux = new Mux(transport);
  mux.start();
  const reader = mux.incoming([0]);
  const pending = reader.u8(); // waiting on inbound bytes that never arrive
  fail(new Error("connection reset"));
  await assert.rejects(pending, /connection reset/);
});

const futures = "wrpc-test:async/futures";
const fut = t.future(t.string);

test("future parameters and results resolve end to end", async () => {
  const [client, server] = duplexPair();

  const serving = (async () => {
    const inv = await accept(server);
    const { params, done: rx } = await inv.receiveParams([fut]);
    const greeting = await params[0]; // the future the client sent
    await Promise.all([rx, inv.sendResults([fut], [Promise.resolve(`${greeting}, world`)])]);
  })();

  const { results, done } = await invoke(
    client,
    futures,
    "greet",
    [fut],
    // a future parameter may be a value, a promise, or a thunk returning one
    [Promise.resolve("hello")],
    [fut],
  );

  assert.equal(await results[0], "hello, world");
  await Promise.all([done, serving]);
});

test("a result-less, async-less invocation completes", async () => {
  const [client, server] = duplexPair();

  const serving = (async () => {
    const inv = await accept(server);
    const { params } = await inv.receiveParams([t.string]);
    assert.equal(params[0], "key");
    await inv.sendResults([], []);
  })();

  const { results, done } = await invoke(client, "wasi:keyvalue/store", "delete", [t.string], ["key"], []);
  assert.deepEqual(results, []);
  await Promise.all([done, serving]);
});

// ----- nested async (stream/future within stream/future) --------------------

/**
 * Invoke an `echo` of a single-parameter, single-result function whose
 * parameter and result share the type `ty`; the server reflects the decoded
 * value straight back, exercising both the decode and re-encode of `ty`.
 */
async function echo(ty, arg) {
  const [client, server] = duplexPair();
  const serving = (async () => {
    const inv = await accept(server);
    const { params, done: rx } = await inv.receiveParams([ty]);
    await Promise.all([rx, inv.sendResults([ty], [params[0]])]);
  })();
  const { results, done } = await invoke(client, "test:nested/echo", "echo", [ty], [arg], [ty]);
  return { result: results[0], done, serving };
}

test("future<future<T>> resolves through both layers", async () => {
  const ty = t.future(t.future(t.string));
  const { result, done, serving } = await echo(ty, "deep");
  assert.equal(await (await result), "deep");
  await Promise.all([done, serving]);
});

test("future<stream<u8>> resolves to a byte stream", async () => {
  const ty = t.future(t.stream(t.u8));
  const { result, done, serving } = await echo(ty, new Uint8Array([1, 2, 3]));
  assert.deepEqual(await collect(await result), [1, 2, 3]);
  await Promise.all([done, serving]);
});

test("stream<future<T>> yields chunks of promises", async () => {
  const ty = t.stream(t.future(t.string));
  const { result, done, serving } = await echo(ty, [["a", "b"], ["c"]]);
  const out = [];
  for await (const chunk of result) out.push(...(await Promise.all(chunk)));
  assert.deepEqual(out, ["a", "b", "c"]);
  await Promise.all([done, serving]);
});

test("stream<stream<u8>> yields chunks of byte streams", async () => {
  const ty = t.stream(t.stream(t.u8));
  // one outer chunk carrying two inner byte streams
  const { result, done, serving } = await echo(ty, [[new Uint8Array([1, 2]), new Uint8Array([3, 4])]]);
  const out = [];
  for await (const chunk of result) {
    for (const inner of chunk) out.push(await collect(inner));
  }
  assert.deepEqual(out, [[1, 2], [3, 4]]);
  await Promise.all([done, serving]);
});
