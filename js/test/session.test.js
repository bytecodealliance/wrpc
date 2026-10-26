import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { Chan, Writer } from "../src/io.js";
import { Mux, fromWebStreams } from "../src/mux.js";
import { invoke, accept } from "../src/session.js";
import { encodeValue } from "../src/value.js";

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
    abort: (reason) => {
      chan.write.close(reason);
      chan.read.cancel(reason);
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
    [
      {
        numbers: [
          [1n, 2n, 3n],
          [4n, 5n],
        ],
        bytes: new Uint8Array([10, 20, 30, 40]),
      },
    ],
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
  mux.seal([], [{ path: [0], nested: false }]);
  const reader = mux.incoming([0]);
  const pending = reader.u8(); // waiting on inbound bytes that never arrive
  fail(new Error("connection reset"));
  await assert.rejects(pending, /connection reset/);
});

test("aborting after the peer closed fails the sub-streams declared later", async () => {
  const mux = new Mux({ read: async () => null, write: () => {} });
  mux.start();
  await mux.ingress;
  mux.abort(new Error("boom"));
  mux.seal([], [{ path: [0], nested: false }]);
  await assert.rejects(mux.incoming([0]).u8(), /boom/);
});

test("a failed invocation leaves no unhandled rejection", async () => {
  const transport = {
    read: () => Promise.reject(new Error("connection refused")),
    write: () => {},
    closeWrite: () => Promise.reject(new Error("connection refused")),
  };
  await assert.rejects(invoke(transport, STREAMS, "get", [], [], [t.u8]), /connection refused/);
});

test("a failed reply stops sending stream parameters", { timeout: 1000 }, async () => {
  let stop = () => {};
  const stopped = new Promise((resolve) => (stop = resolve));
  async function* numbers() {
    try {
      for (;;) {
        yield [1n];
        await new Promise((resolve) => setImmediate(resolve));
      }
    } finally {
      stop();
    }
  }
  const transport = { read: async () => null, write: () => {} };
  await assert.rejects(
    invoke(transport, STREAMS, "echo", [t.stream(t.u64)], [numbers()], [t.u8]),
    /unexpected end of input/,
  );
  await stopped;
});

test("a failed parameter write takes precedence over the reply's failure", async () => {
  let writes = 0;
  const transport = {
    read: () => new Promise((resolve) => setTimeout(() => resolve(null), 10)),
    write: () => (++writes > 2 ? Promise.reject(new Error("broken pipe")) : undefined),
  };
  await assert.rejects(
    invoke(transport, STREAMS, "echo", [t.stream(t.u8)], [new Uint8Array([1])], [t.u8]),
    /broken pipe/,
  );
});

test("a failed parameter stream aborts the transport", { timeout: 1000 }, async () => {
  let aborted;
  let release = () => {};
  const transport = {
    read: () => new Promise((resolve) => (release = resolve)),
    write: () => {},
    abort: (reason) => {
      aborted = reason;
      release(null);
    },
  };
  async function* numbers() {
    yield [1n];
    throw new Error("source failed");
  }
  await assert.rejects(
    invoke(transport, STREAMS, "echo", [t.stream(t.u64)], [numbers()], [t.u8]),
    /source failed/,
  );
  assert.match(aborted.message, /source failed/);
});

test("an invalid invocation header aborts the transport", async () => {
  let aborted;
  const transport = {
    read: async () => Uint8Array.of(0x42),
    write: () => {},
    abort: (reason) => void (aborted = reason),
  };
  await assert.rejects(accept(transport), /unsupported wRPC framing version byte 0x42/);
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(aborted.message, /unsupported/);
});

test("aborting a web stream transport cancels and aborts both sides", async () => {
  const reasons = [];
  const transport = fromWebStreams({
    readable: new ReadableStream({ cancel: (reason) => void reasons.push(["cancel", reason]) }),
    writable: new WritableStream({ abort: (reason) => void reasons.push(["abort", reason]) }),
  });
  await transport.abort("boom");
  assert.deepEqual(reasons.sort(), [
    ["abort", "boom"],
    ["cancel", "boom"],
  ]);
});

test("frame data is routed as it arrives", async () => {
  const bytes = [0x01, 0x00, 0x03, 0x0a, 0x14, 0x1e, 0x01, 0x00, 0x02, 0x28];
  const transport = {
    read: async () => (bytes.length > 0 ? new Uint8Array(bytes.splice(0, 1)) : null),
    write: () => {},
  };
  const mux = new Mux(transport);
  mux.start();
  mux.seal([], [{ path: [0], nested: false }]);
  const reader = mux.incoming([0]);
  assert.deepEqual([...(await reader.take(4))], [10, 20, 30, 40]);
  await assert.rejects(reader.take(1), /unexpected end of input/);
});

test("a value split across many frames and chunks decodes", async () => {
  const frames = [];
  const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
  const big = new Uint8Array(70000).map((_, i) => i);
  const w = new Writer();
  encodeValue(w, t.string, "hello");
  encodeValue(w, t.list(t.u8), big);
  const reply = w.finish();
  for (let i = 0; i < reply.length; i += 1000)
    await peer.outgoing([]).write(reply.subarray(i, i + 1000));
  const bytes = frames.flatMap((frame) => [...frame]);
  const transport = {
    read: async () => (bytes.length > 0 ? new Uint8Array(bytes.splice(0, 7)) : null),
    write: () => {},
  };
  const { results, done } = await invoke(
    transport,
    STREAMS,
    "get",
    [],
    [],
    [t.string, t.list(t.u8)],
  );
  assert.equal(results[0], "hello");
  assert.deepEqual(results[1], big);
  await done;
});

test("invalid arguments fail before anything is written and abort the transport", async () => {
  const writes = [];
  const aborted = [];
  const transport = {
    read: async () => null,
    write: (bytes) => void writes.push(bytes),
    abort: (reason) => void aborted.push(reason),
  };
  await assert.rejects(invoke(transport, STREAMS, "get", [t.u32], [-1], []), /out of range/);
  await assert.rejects(invoke(transport, STREAMS, "get", [t.u32], [], []), /expected 1 argument/);
  const ty = t.list(t.future(t.u32));
  await assert.rejects(invoke(transport, STREAMS, "get", [ty], [[1]], []), /not supported/);
  await new Promise(setImmediate);
  assert.deepEqual(writes, []);
  assert.equal(aborted.length, 3);
});

test("invalid stream chunks fail the invocation", async () => {
  for (const [ty, chunks, error] of [
    [t.stream(t.u8), [1, 2, 3], /expected `Uint8Array`/],
    [t.stream(t.future(t.string)), [["foo"]], /not supported/],
  ]) {
    const transport = {
      read: () => new Promise((resolve) => setTimeout(() => resolve(null), 10)),
      write: () => {},
    };
    await assert.rejects(invoke(transport, STREAMS, "echo", [ty], [chunks], [t.u8]), error);
  }
});

for (const [name, values, error] of [
  ["a mismatched number of", [], /expected 1 result/],
  ["mistyped", [1], /expected a string/],
]) {
  test(`sending ${name} results fails the invocation`, { timeout: 1000 }, async () => {
    const [client, server] = duplexPair();
    const serving = (async () => {
      const inv = await accept(server);
      await inv.receiveParams([]);
      await assert.rejects(inv.sendResults([t.string], values), error);
    })();
    await assert.rejects(invoke(client, STREAMS, "get", [], [], [t.string]));
    await serving;
  });
}

/**
 * A transport replaying the frames of a reply carrying one pending
 * `stream<u8>` result followed by `n` one-byte chunks of it, and counting
 * the reads.
 */
async function streamingReply(n) {
  const frames = [];
  const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
  await peer.outgoing([]).write(Uint8Array.of(0));
  for (let i = 0; i < n; i++) await peer.outgoing([0]).write(Uint8Array.of(1, i & 0xff));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  const transport = {
    reads: 0,
    read: async () => {
      transport.reads++;
      return frames.shift() ?? null;
    },
    write: () => {},
  };
  return transport;
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

test("an unconsumed stream result stops reading the transport", { timeout: 1000 }, async () => {
  const transport = await streamingReply(1000);
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  for (let i = 0; i < 100; i++) await tick();
  assert.ok(transport.reads < 200, `read ${transport.reads} frames`);
  assert.equal((await collect(results[0])).length, 1000);
  await done;
  assert.equal(transport.reads, 1003);
});

test("a cancelled stream result is discarded", { timeout: 1000 }, async () => {
  const transport = await streamingReply(1000);
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  for await (const chunk of results[0]) {
    assert.deepEqual(chunk, Uint8Array.of(0));
    break;
  }
  await done;
  // The rest of the stream is still read off the transport, and dropped.
  while (transport.reads < 1003) await tick();
  assert.deepEqual(await results[0].next(), { value: undefined, done: true });
});

test("data arriving before its sub-stream is decoded is kept", { timeout: 1000 }, async () => {
  const frames = [];
  const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
  await peer.outgoing([0]).write(Uint8Array.of(1, 7));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  await peer.outgoing([]).write(Uint8Array.of(0));
  const transport = { read: async () => frames.shift() ?? null, write: () => {} };
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  assert.deepEqual(await collect(results[0]), [7]);
  await done;
});

test(
  "data arriving before sibling sub-streams are decoded is kept",
  { timeout: 1000 },
  async () => {
    const frames = [];
    const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
    await peer.outgoing([0, 1]).write(Uint8Array.of(1, 8));
    await peer.outgoing([0, 1]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(1, 7));
    await peer.outgoing([0, 0]).write(Uint8Array.of(0));
    await peer.outgoing([]).write(Uint8Array.of(0, 0));
    const transport = { read: async () => frames.shift() ?? null, write: () => {} };
    const ty = [t.tuple(t.stream(t.u8), t.stream(t.u8))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    assert.deepEqual(await Promise.all(results[0].map(collect)), [[7], [8]]);
    await done;
  },
);

test(
  "more data than a sub-stream buffers arriving ahead of its declaration is kept",
  { timeout: 1000 },
  async () => {
    const n = 40;
    const frames = [];
    const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
    await peer.outgoing([]).write(Uint8Array.of(0));
    for (let i = 0; i < n; i++) await peer.outgoing([0, 0]).write(Uint8Array.of(1, i & 0xff));
    await peer.outgoing([0, 0]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    const transport = { read: async () => frames.shift() ?? null, write: () => {} };
    const ty = [t.future(t.tuple(t.stream(t.u8)))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    assert.deepEqual(
      await collect(inner),
      Array.from({ length: n }, (_, i) => i & 0xff),
    );
    await done;
  },
);
test("an invocation sends no frames for empty data", async () => {
  for (const [types, args, expected] of [
    [[], [], "\x00\x03foo\x03bar"],
    [[t.future()], [Promise.resolve()], "\x00\x03foo\x03bar\x00\x01\x00\x01\x00\x00"],
  ]) {
    const frames = [];
    let closed = false;
    const transport = {
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => void (closed = true),
    };
    const { done } = await invoke(transport, "foo", "bar", types, args, []);
    await done;
    assert.ok(closed);
    assert.deepEqual(Buffer.concat(frames), Buffer.from(expected, "latin1"));
  }
});

test("writing empty data fails once aborted", async () => {
  const mux = new Mux({ read: async () => null, write: () => {} });
  mux.abort(new Error("boom"));
  await assert.rejects(mux.outgoing([]).write(new Uint8Array(0)), /boom/);
});

/** A transport fed by the frames a peer writes, until `inbound` is closed. */
function feed() {
  const inbound = new Chan();
  const peer = new Mux({ read: async () => null, write: (bytes) => void inbound.push(bytes) });
  /** @type {(reason: unknown) => void} */
  let abort = () => {};
  const aborted = new Promise((resolve) => (abort = resolve));
  const transport = {
    read: async () => (await inbound.next()).value ?? null,
    write: () => {},
    abort,
  };
  return { inbound, peer, transport, aborted };
}

test("a future without a payload resolves at the end of the transport", async () => {
  for (const data of [false, true]) {
    const { inbound, peer, transport } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    if (data) await peer.outgoing([0]).write(Uint8Array.of(0));
    inbound.close();
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.future()]);
    if (data) {
      await assert.rejects(results[0], /unexpected data for sub-stream \[0\]/);
      await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
    } else {
      assert.equal(await results[0], undefined);
      await done;
    }
  }
});

test("an empty frame for a finished sub-stream is accepted", { timeout: 1000 }, async () => {
  const { inbound, peer, transport, aborted } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(1, 7));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  assert.deepEqual(await collect(results[0]), [7]);
  await done;
  await peer.outgoing([0]).close();
  inbound.close();
  const outcome = await Promise.race([
    aborted,
    tick()
      .then(tick)
      .then(() => "ok"),
  ]);
  assert.equal(outcome, "ok");
});

test(
  "an empty frame arriving before its sub-stream is declared closes it",
  { timeout: 1000 },
  async () => {
    const frames = [];
    const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).close();
    await peer.outgoing([0]).write(Uint8Array.of(0));
    const transport = { read: async () => frames.shift() ?? null, write: () => {} };
    const ty = [t.future(t.tuple(t.future()))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    assert.equal(await inner, undefined);
    await done;
  },
);
test("frame data beyond the end of a sub-stream fails it", { timeout: 1000 }, async () => {
  for (const [ty, value, extra, split] of [
    [t.future(t.u8), [5], 40, true],
    [t.stream(t.u8), [1, 5, 0], 40, true],
    [t.future(t.u8), [5], 3, true],
    [t.stream(t.u8), [1, 5, 0], 3, true],
    [t.future(t.u8), [5], 3, false],
    [t.stream(t.u8), [1, 5, 0], 3, false],
  ]) {
    const w = new Writer();
    w.varU(1);
    w.varU(0);
    w.varU(value.length + extra);
    w.bytes([...value, ...new Array(extra).fill(9)]);
    const frame = w.finish();
    const bytes = [0, 1, 0, ...frame];
    const chunks = split ? bytes.map((b) => Uint8Array.of(b)) : [Uint8Array.from(bytes)];
    /** @type {(reason: unknown) => void} */
    let abort = () => {};
    const aborted = new Promise((resolve) => (abort = resolve));
    const transport = { read: async () => chunks.shift() ?? null, write: () => {}, abort };
    const { results } = await invoke(transport, STREAMS, "get", [], [], [ty]);
    assert.deepEqual(ty.kind === "future" ? [await results[0]] : await collect(results[0]), [5]);
    assert.match(String(await aborted), /unexpected data for sub-stream \[0\]/);
  }
});

test(
  "a sub-stream buffered ahead of its declaration is bounded once declared",
  { timeout: 1000 },
  async () => {
    const frames = [];
    const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(1, 0));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    for (let i = 0; i < 1000; i++) await peer.outgoing([0, 0]).write(Uint8Array.of(1, i & 0xff));
    let reads = 0;
    const transport = { read: async () => (reads++, frames.shift() ?? null), write: () => {} };
    const ty = [t.future(t.tuple(t.stream(t.u8)))];
    const { results } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    await inner[Symbol.asyncIterator]().next();
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.ok(reads < 500, `read ${reads} frames`);
  },
);
test("data for the root after the reply fails it", { timeout: 1000 }, async () => {
  const frames = [];
  const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
  await peer.outgoing([]).write(Uint8Array.of(0));
  for (let i = 0; i < 40; i++) await peer.outgoing([]).write(Uint8Array.of(i));
  await peer.outgoing([0]).write(Uint8Array.of(1, 7));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  const transport = { read: async () => frames.shift() ?? null, write: () => {} };
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  await assert.rejects(collect(results[0]), /unexpected data for sub-stream \[\]/);
  await assert.rejects(done, /unexpected data for sub-stream \[\]/);
});

test("a truncated stream fails", { timeout: 1000 }, async () => {
  for (const send of [
    (peer, frames) => frames.push(Uint8Array.of(1, 0, 5, 1, 6)),
    (peer) => peer.outgoing([0]).write(Uint8Array.of(1, 5, 0x80)),
    async (peer) => {
      await peer.outgoing([0]).write(Uint8Array.of(1, 7));
      await peer.outgoing([0]).close();
    },
  ]) {
    const frames = [];
    const peer = new Mux({ read: async () => null, write: (bytes) => void frames.push(bytes) });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await send(peer, frames);
    const transport = { read: async () => frames.shift() ?? null, write: () => {} };
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
    await assert.rejects(collect(results[0]), /unexpected end of input/);
    await assert.rejects(done, /unexpected end of input/);
  }
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

  const { results, done } = await invoke(
    client,
    "wasi:keyvalue/store",
    "delete",
    [t.string],
    ["key"],
    [],
  );
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
  assert.equal(await await result, "deep");
  await Promise.all([done, serving]);
});

test("future<stream<u8>> resolves to a byte stream", async () => {
  const ty = t.future(t.stream(t.u8));
  const { result, done, serving } = await echo(ty, new Uint8Array([1, 2, 3]));
  assert.deepEqual(await collect(await result), [1, 2, 3]);
  await Promise.all([done, serving]);
});

for (const [kind, ty, chunk] of [
  ["list", t.list(t.future(t.u8)), Uint8Array.of(1, 0)],
  ["stream", t.stream(t.future(t.u8)), Uint8Array.of(0)],
]) {
  test(`pending values in ${kind} elements are not supported`, { timeout: 1000 }, async () => {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([]).write(chunk);
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    inbound.close();
    const invoked = invoke(transport, STREAMS, "get", [], [], [ty]);
    if (kind === "stream")
      await assert.rejects(collect((await invoked).results[0]), /not supported/);
    else await assert.rejects(invoked, /not supported/);
    assert.match(String(await aborted), /not supported/);
  });
}

test("data nested in list elements of a future fails it", { timeout: 1000 }, async () => {
  const { peer, transport, aborted } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0, 0]).write(Uint8Array.of(7));
  await invoke(transport, STREAMS, "get", [], [], [t.future(t.list(t.future(t.u8)))]);
  assert.match(String(await aborted), /unexpected data for sub-stream \[0,0\]/);
});

test("ready values in stream elements are received", { timeout: 1000 }, async () => {
  const { inbound, peer, transport } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(2, 1, 5, 1, 6));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  inbound.close();
  const ty = [t.stream(t.future(t.u8))];
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
  assert.deepEqual(await Promise.all(await collect(results[0])), [5, 6]);
  await done;
});

for (const [name, frame] of [
  ["data", (out) => out.write(Uint8Array.of(7))],
  ["an empty frame", (out) => out.close()],
]) {
  test(
    `${name} for a sub-stream the reply does not carry fails it`,
    { timeout: 1000 },
    async () => {
      const { inbound, peer, transport, aborted } = feed();
      await peer.outgoing([]).write(Uint8Array.of(0, 0));
      await frame(peer.outgoing([0]));
      await peer.outgoing([1]).write(Uint8Array.of(1, 7, 0));
      inbound.close();
      const ty = [t.option(t.stream(t.u8)), t.stream(t.u8)];
      const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
      await assert.rejects(collect(results[1]), /unexpected data for sub-stream \[0\]/);
      await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
      assert.match(String(await aborted), /unexpected data/);
    },
  );

  for (const [declared, path, ty, init] of [
    [true, [0], t.stream(t.u8), [[[0], Uint8Array.of(1, 7, 0)]]],
    [false, [0, 0], t.future(t.tuple(t.future(t.u8))), []],
  ]) {
    test(
      `${name} for a closed sub-stream ${declared ? "" : "not yet declared "}fails it`,
      { timeout: 1000 },
      async () => {
        const { inbound, peer, transport, aborted } = feed();
        await peer.outgoing([]).write(Uint8Array.of(0));
        for (const [p, bytes] of init) await peer.outgoing(p).write(bytes);
        await peer.outgoing(path).close();
        await frame(peer.outgoing(path));
        if (!declared) await peer.outgoing([0]).write(Uint8Array.of(0));
        inbound.close();
        await invoke(transport, STREAMS, "get", [], [], [ty]);
        assert.match(
          String(await aborted),
          new RegExp(`unexpected data for sub-stream \\[${path}\\]`),
        );
      },
    );
  }
}
