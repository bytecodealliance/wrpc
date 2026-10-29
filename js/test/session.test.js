import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

import * as t from "../src/types.js";
import { Chan, Writer } from "../src/io.js";
import { Mux, fromWebStreams } from "../src/mux.js";
import { invoke, accept } from "../src/session.js";
import { encodeValue } from "../src/value.js";

/**
 * @typedef {import("../src/index.js").Chunk} Chunk
 * @typedef {import("../src/index.js").InputValue} InputValue
 * @typedef {import("../src/index.js").Transport} Transport
 * @typedef {import("../src/index.js").Type} Type
 * @typedef {import("../src/index.js").TypedArray} TypedArray
 * @typedef {{ new (length: number): TypedArray, from(vals: Iterable<number | bigint>): TypedArray, readonly name: string }} TypedArrayConstructor
 */

/**
 * A pair of in-memory transports wired back to back, modelling a single
 * bidirectional byte stream per invocation.
 */
function duplexPair() {
  const a = new Chan();
  const b = new Chan();
  /**
   * @param {{ read: Chan<Uint8Array>, write: Chan<Uint8Array> }} chan
   * @returns {Transport}
   */
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

/**
 * Drain an async iterable of chunks into a flat array.
 * @param {AsyncIterable<Chunk>} stream
 */
async function collect(stream) {
  const out = [];
  for await (const chunk of stream) out.push(...chunk);
  return out;
}

/**
 * Assert that `stream`, once drained, stays pending instead of ending.
 * @param {AsyncIterable<Chunk>} stream
 */
async function assertPending(stream) {
  const pending = Symbol("pending");
  const end = new Promise((resolve) => setImmediate(resolve, pending));
  assert.equal(await Promise.race([collect(stream), end]), pending);
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

test("numeric stream chunks are typed arrays", async () => {
  for (const [
    elem,
    Typed,
    vals,
  ] of /** @type {[Type, TypedArrayConstructor, (number | bigint)[]][]} */ ([
    [t.s8, Int8Array, [-128, 0, 127]],
    [t.u8, Uint8Array, [0, 1, 255]],
    [t.s16, Int16Array, [-32768, 1, 32767]],
    [t.u16, Uint16Array, [0, 1, 65535]],
    [t.s32, Int32Array, [-2147483648, 1, 2147483647]],
    [t.u32, Uint32Array, [0, 1, 4294967295]],
    [t.s64, BigInt64Array, [-(1n << 63n), 1n, (1n << 63n) - 1n]],
    [t.u64, BigUint64Array, [0n, 1n, (1n << 64n) - 1n]],
    [t.f32, Float32Array, [1.5, -0.25, Infinity]],
    [t.f64, Float64Array, [Math.PI, 5e-324, -Infinity]],
  ])) {
    const ty = t.stream(elem);
    for (const [src, chunks] of /** @type {[InputValue, TypedArray[]][]} */ ([
      [Typed.from(vals), [Typed.from(vals)]],
      [
        [vals.slice(0, 1), Typed.from(vals.slice(1)), []],
        [Typed.from(vals.slice(0, 1)), Typed.from(vals.slice(1))],
      ],
      [runInNewContext(`${Typed.name}.from(vals)`, { vals }), [Typed.from(vals)]],
      [[runInNewContext(`${Typed.name}.from(vals)`, { vals })], [Typed.from(vals)]],
      [[new Set(vals)], [Typed.from(vals)]],
    ])) {
      const [client, server] = duplexPair();
      const serving = (async () => {
        const inv = await accept(server);
        const { params, done: rx } = await inv.receiveParams([ty]);
        await Promise.all([rx, inv.sendResults([ty], [params[0]])]);
      })();
      const { results, done } = await invoke(client, STREAMS, "echo", [ty], [src], [ty]);
      const out = [];
      for await (const chunk of results[0]) {
        assert.ok(chunk instanceof Typed, `\`stream<${elem.kind}>\` chunks are \`${Typed.name}\``);
        out.push(chunk);
      }
      assert.deepEqual(out, chunks);
      await Promise.all([done, serving]);
    }
  }
});

test("an inbound transport failure surfaces to a sub-stream reader", async () => {
  /** @type {(e: Error) => void} */
  let fail = () => {};
  const transport = {
    read: () => new Promise((_, reject) => (fail = reject)),
    write: () => {},
    closeWrite: () => {},
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
  const mux = new Mux({ read: async () => null, write: () => {}, closeWrite: () => {} });
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
  /** @type {(value?: unknown) => void} */
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
  const transport = { read: async () => null, write: () => {}, closeWrite: () => {} };
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
    closeWrite: () => {},
  };
  await assert.rejects(
    invoke(transport, STREAMS, "echo", [t.stream(t.u8)], [new Uint8Array([1])], [t.u8]),
    /broken pipe/,
  );
});

test("a failed parameter stream aborts the transport", { timeout: 1000 }, async () => {
  /** @type {unknown} */
  let aborted;
  /** @type {(value: null) => void} */
  let release = () => {};
  /** @type {Transport} */
  const transport = {
    read: () => new Promise((resolve) => (release = resolve)),
    write: () => {},
    closeWrite: () => {},
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
  assert.match(String(aborted), /source failed/);
});

test("an invalid invocation header aborts the transport", async () => {
  /** @type {unknown} */
  let aborted;
  /** @type {Transport} */
  const transport = {
    read: async () => Uint8Array.of(0x42),
    write: () => {},
    closeWrite: () => {},
    abort: (reason) => void (aborted = reason),
  };
  await assert.rejects(accept(transport), /unsupported wRPC framing version byte 0x42/);
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(String(aborted), /unsupported/);
});

test("aborting a web stream transport cancels and aborts both sides", async () => {
  for (const [reason, expected] of [
    ["boom", "boom"],
    [Object.assign(new Error("stopped"), { source: "stream", streamErrorCode: 1 }), undefined],
  ]) {
    /** @type {[string, unknown][]} */
    const reasons = [];
    const transport = fromWebStreams({
      readable: new ReadableStream({ cancel: (reason) => void reasons.push(["cancel", reason]) }),
      writable: new WritableStream({ abort: (reason) => void reasons.push(["abort", reason]) }),
    });
    assert.ok(transport.abort);
    await transport.abort(reason);
    assert.deepEqual(reasons.sort(), [
      ["abort", expected],
      ["cancel", expected],
    ]);
  }
});

/**
 * Invoke a function returning a `u8` over a web stream transport whose
 * writable side the peer stops with `code` while it is being closed.
 *
 * @param {number} code
 */
function invokeStopped(code) {
  const transport = fromWebStreams({
    readable: new ReadableStream({
      start: (controller) => {
        controller.enqueue(Uint8Array.of(0x00, 0x01, 0x2a));
        controller.close();
      },
    }),
    writable: new WritableStream({
      close: () =>
        Promise.reject(
          Object.assign(new Error("Received STOP_SENDING."), {
            name: "WebTransportError",
            source: "stream",
            streamErrorCode: code,
          }),
        ),
    }),
  });
  return invoke(transport, STREAMS, "get", [], [], [t.u8]);
}

test("the peer stopping the closing web stream with code 1 completes it", async () => {
  const { results, done } = await invokeStopped(1);
  assert.deepEqual(results, [42]);
  await done;
});

test("the peer stopping the closing web stream with another code fails it", async () => {
  const { results, done } = await invokeStopped(0);
  assert.deepEqual(results, [42]);
  await assert.rejects(done, { name: "WebTransportError", streamErrorCode: 0 });
});

test("frame data is routed as it arrives", async () => {
  const bytes = [0x01, 0x00, 0x03, 0x0a, 0x14, 0x1e, 0x01, 0x00, 0x02, 0x28];
  const transport = {
    read: async () => (bytes.length > 0 ? new Uint8Array(bytes.splice(0, 1)) : null),
    write: () => {},
    closeWrite: () => {},
  };
  const mux = new Mux(transport);
  mux.start();
  mux.seal([], [{ path: [0], nested: false }]);
  const reader = mux.incoming([0]);
  assert.deepEqual([...(await reader.take(4))], [10, 20, 30, 40]);
  await assert.rejects(reader.take(1), /unexpected end of input/);
});

test("a value split across many frames and chunks decodes", async () => {
  /** @type {Uint8Array[]} */
  const frames = [];
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void frames.push(bytes),
    closeWrite: () => {},
  });
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
    closeWrite: () => {},
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
  /** @type {Uint8Array[]} */
  const writes = [];
  /** @type {unknown[]} */
  const aborted = [];
  /** @type {Transport} */
  const transport = {
    read: async () => null,
    write: (bytes) => void writes.push(bytes),
    closeWrite: () => {},
    abort: (reason) => void aborted.push(reason),
  };
  await assert.rejects(invoke(transport, STREAMS, "get", [t.u32], [1n], []), TypeError);
  await assert.rejects(invoke(transport, STREAMS, "get", [t.u32], [], []), /expected 1 argument/);
  await new Promise(setImmediate);
  assert.deepEqual(writes, []);
  assert.equal(aborted.length, 2);
});

test("invalid stream chunks fail the invocation", async () => {
  for (const [
    ty,
    chunks,
    error,
  ] of /** @type {[Type, InputValue, RegExp | typeof TypeError][]} */ ([
    [t.stream(t.u8), [1, 2, 3], /expected an iterable/],
    [t.stream(t.u8), [Uint8Array.of(1).buffer], /expected an iterable/],
    [t.stream(t.u16), Int32Array.of(1), /expected an iterable/],
    [t.stream(t.u16), [[1n]], TypeError],
  ])) {
    const transport = {
      read: () => new Promise((resolve) => setTimeout(() => resolve(null), 10)),
      write: () => {},
      closeWrite: () => {},
    };
    await assert.rejects(invoke(transport, STREAMS, "echo", [ty], [chunks], [t.u8]), error);
  }
});

for (const [name, values, error] of /** @type {[string, unknown[], RegExp][]} */ ([
  ["a mismatched number of", [], /expected 1 result/],
  ["mistyped", [Symbol()], /Symbol/],
])) {
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
 * @param {number} n
 */
async function streamingReply(n) {
  /** @type {Uint8Array[]} */
  const frames = [];
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void frames.push(bytes),
    closeWrite: () => {},
  });
  await peer.outgoing([]).write(Uint8Array.of(0));
  for (let i = 0; i < n; i++) await peer.outgoing([0]).write(Uint8Array.of(1, i & 0xff));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  /** @type {Transport & { reads: number }} */
  const transport = {
    reads: 0,
    read: async () => {
      transport.reads++;
      return frames.shift() ?? null;
    },
    write: () => {},
    closeWrite: () => {},
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

test(
  "a cancelled stream result with data left fails the invocation",
  { timeout: 1000 },
  async () => {
    const transport = await streamingReply(1000);
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
    for await (const chunk of results[0]) {
      assert.deepEqual(chunk, Uint8Array.of(0));
      break;
    }
    await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
    assert.deepEqual(await results[0].next(), { value: undefined, done: true });
  },
);

test("data arriving before its sub-stream is decoded is kept", { timeout: 1000 }, async () => {
  /** @type {Uint8Array[]} */
  const frames = [];
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void frames.push(bytes),
    closeWrite: () => {},
  });
  await peer.outgoing([0]).write(Uint8Array.of(1, 7));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  await peer.outgoing([]).write(Uint8Array.of(0));
  const transport = {
    read: async () => frames.shift() ?? null,
    write: () => {},
    closeWrite: () => {},
  };
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  assert.deepEqual(await collect(results[0]), [7]);
  await done;
});

test("data for results which cannot be pending fails at once", { timeout: 1000 }, async () => {
  /** @type {Uint8Array[]} */
  const frames = [];
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void frames.push(bytes),
    closeWrite: () => {},
  });
  await peer.outgoing([2]).close();
  await peer.outgoing([0]).write(Uint8Array.of(0));
  const transport = {
    read: async () => frames.shift() ?? null,
    write: () => {},
    closeWrite: () => {},
  };
  await assert.rejects(
    invoke(transport, STREAMS, "get", [], [], [t.u32, t.stream(t.u8)]),
    /unexpected data for sub-stream \[0\]/,
  );
});

test(
  "data arriving before sibling sub-streams are decoded is kept",
  { timeout: 1000 },
  async () => {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    await peer.outgoing([0, 1]).write(Uint8Array.of(1, 8));
    await peer.outgoing([0, 1]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(1, 7));
    await peer.outgoing([0, 0]).write(Uint8Array.of(0));
    await peer.outgoing([]).write(Uint8Array.of(0, 0));
    const transport = {
      read: async () => frames.shift() ?? null,
      write: () => {},
      closeWrite: () => {},
    };
    const ty = [t.tuple(t.stream(t.u8), t.stream(t.u8))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    assert.deepEqual(await Promise.all(results[0].map(collect)), [[7], [8]]);
    await done;
  },
);

test(
  "data for a sub-stream declared while ingress waits for room is routed",
  { timeout: 1000 },
  async () => {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    for (let k = 0; k < 64; k++) await peer.outgoing([1, k]).write(Uint8Array.of(7));
    await peer.outgoing([0, 0]).write(Uint8Array.of(42));
    const mux = new Mux({
      read: async () => frames.shift() ?? new Promise(() => {}),
      write: () => {},
      closeWrite: () => {},
    });
    mux.seal(
      [],
      [
        { path: [0], nested: true },
        { path: [1], nested: true },
      ],
    );
    mux.start();
    while (!mux.pendingWaiters.length) await tick();
    mux.seal([0, 0], [{ path: [], nested: false }]);
    const ar = mux.incoming([0, 0]);
    assert.ok(await ar.fill(1));
    assert.deepEqual(ar.remaining(), Uint8Array.of(42));
  },
);

test("an invocation sends no frames for empty data", async () => {
  for (const [types, args, expected] of /** @type {[Type[], InputValue[], string][]} */ ([
    [[], [], "\x00\x03foo\x03bar"],
    [[t.future()], [Promise.resolve()], "\x00\x03foo\x03bar\x00\x01\x00\x01\x00\x00"],
  ])) {
    /** @type {Uint8Array[]} */
    const frames = [];
    let closed = false;
    /** @type {Transport} */
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

test("inbound frame data is buffered in chunks of at most 1 MiB", async () => {
  const w = new Writer();
  w.varU(0);
  w.varU(3 << 20);
  w.bytes(new Uint8Array(3 << 20));
  /** @type {Uint8Array[]} */
  const bytes = [w.finish()];
  const mux = new Mux({
    read: async () => bytes.pop() ?? null,
    write: () => {},
    closeWrite: () => {},
  });
  mux.start();
  const chan = mux.root.chan;
  assert.ok(chan);
  const sizes = [];
  for (let n = 0; n < 3 << 20; n += sizes[sizes.length - 1]) {
    const { value } = await chan.next();
    assert.ok(value);
    sizes.push(value.length);
  }
  assert.deepEqual(sizes, [1 << 20, 1 << 20, 1 << 20]);
});

test("writing a frame overflowing a u32 fails", async () => {
  const mux = new Mux({ read: async () => null, write: () => {}, closeWrite: () => {} });
  await assert.rejects(mux.outgoing([2 ** 32]).write(Uint8Array.of(1)), RangeError);
});

test("writing empty data fails once aborted", async () => {
  const mux = new Mux({ read: async () => null, write: () => {}, closeWrite: () => {} });
  mux.abort(new Error("boom"));
  await assert.rejects(mux.outgoing([]).write(new Uint8Array(0)), /boom/);
});

/** A transport fed by the frames a peer writes, until `inbound` is closed. */
function feed() {
  const inbound = new Chan();
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void inbound.push(bytes),
    closeWrite: () => {},
  });
  /** @type {(reason: unknown) => void} */
  let abort = () => {};
  const aborted = new Promise((resolve) => (abort = resolve));
  const transport = {
    read: async () => (await inbound.next()).value ?? null,
    write: () => {},
    closeWrite: () => {},
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
      await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
      const pending = Symbol("pending");
      assert.equal(await Promise.race([results[0], pending]), pending);
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
  "an empty frame for a path nested in a forgotten sub-stream does not close it",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(7));
    const ty = [t.future(t.tuple(t.future(t.u8)))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    assert.equal(await inner, 7);
    await done;
    await peer.outgoing([0, 0]).close();
    await peer.outgoing([0]).close();
    inbound.close();
    const outcome = await Promise.race([
      aborted,
      tick()
        .then(tick)
        .then(() => "ok"),
    ]);
    assert.equal(outcome, "ok");
  },
);

test(
  "an empty frame arriving before its sub-stream is declared closes it",
  { timeout: 1000 },
  async () => {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).close();
    await peer.outgoing([0]).write(Uint8Array.of(0));
    const transport = {
      read: async () => frames.shift() ?? null,
      write: () => {},
      closeWrite: () => {},
    };
    const ty = [t.future(t.tuple(t.future()))];
    const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    assert.equal(await inner, undefined);
    await done;
  },
);
test("frame data beyond the end of a sub-stream fails it", { timeout: 1000 }, async () => {
  for (const [ty, value, extra, split] of /** @type {[Type, number[], number, boolean][]} */ ([
    [t.future(t.u8), [5], 40, true],
    [t.stream(t.u8), [1, 5, 0], 40, true],
    [t.future(t.u8), [5], 3, true],
    [t.stream(t.u8), [1, 5, 0], 3, true],
    [t.future(t.u8), [5], 3, false],
    [t.stream(t.u8), [1, 5, 0], 3, false],
  ])) {
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
    const transport = {
      read: async () => chunks.shift() ?? null,
      write: () => {},
      closeWrite: () => {},
      abort,
    };
    const { results, finished } = await invoke(transport, STREAMS, "get", [], [], [ty]);
    assert.deepEqual(ty.kind === "future" ? [await results[0]] : await collect(results[0]), [5]);
    assert.match(String(await aborted), /unexpected data for sub-stream \[0\]/);
    await assert.rejects(finished, /unexpected data for sub-stream \[0\]/);
  }
});

test(
  "a sub-stream buffered ahead of its declaration is bounded once declared",
  { timeout: 1000 },
  async () => {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(1, 0));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    for (let i = 0; i < 1000; i++) await peer.outgoing([0, 0]).write(Uint8Array.of(1, i & 0xff));
    let reads = 0;
    const transport = {
      read: async () => (reads++, frames.shift() ?? null),
      write: () => {},
      closeWrite: () => {},
    };
    const ty = [t.future(t.tuple(t.stream(t.u8)))];
    const { results } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [inner] = await results[0];
    await inner[Symbol.asyncIterator]().next();
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.ok(reads < 500, `read ${reads} frames`);
  },
);
test("data for the root after the reply fails it", { timeout: 1000 }, async () => {
  /** @type {Uint8Array[]} */
  const frames = [];
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void frames.push(bytes),
    closeWrite: () => {},
  });
  await peer.outgoing([]).write(Uint8Array.of(0));
  for (let i = 0; i < 40; i++) await peer.outgoing([]).write(Uint8Array.of(i));
  await peer.outgoing([0]).write(Uint8Array.of(1, 7));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  const transport = {
    read: async () => frames.shift() ?? null,
    write: () => {},
    closeWrite: () => {},
  };
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  await assert.rejects(done, /unexpected data for sub-stream \[\]/);
  await assertPending(results[0]);
});

test("a truncated stream fails", { timeout: 1000 }, async () => {
  for (const send of /** @type {((peer: Mux, frames: Uint8Array[]) => unknown)[]} */ ([
    (peer, frames) => frames.push(Uint8Array.of(1, 0, 5, 1, 6)),
    (peer) => peer.outgoing([0]).write(Uint8Array.of(1, 5, 0x80)),
    async (peer) => {
      await peer.outgoing([0]).write(Uint8Array.of(1, 7));
      await peer.outgoing([0]).close();
    },
  ])) {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await send(peer, frames);
    const transport = {
      read: async () => frames.shift() ?? null,
      write: () => {},
      closeWrite: () => {},
    };
    const { results, done, finished } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.stream(t.u8)],
    );
    await assert.rejects(done, /unexpected end of input/);
    await assert.rejects(finished, /unexpected end of input/);
    await assertPending(results[0]);
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
 * @param {Type} ty @param {InputValue} arg
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

test("ready values in stream elements are received", { timeout: 1000 }, async () => {
  const { inbound, peer, transport } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(2, 1, 5, 1, 6));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  inbound.close();
  const ty = [t.stream(t.future(t.u8))];
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
  const values = /** @type {Promise<number>[]} */ (await collect(results[0]));
  assert.deepEqual(await Promise.all(values), [5, 6]);
  await done;
});

test("data for a sub-stream the reply does not carry fails it", { timeout: 1000 }, async () => {
  const { inbound, peer, transport, aborted } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0, 0));
  await peer.outgoing([0]).write(Uint8Array.of(7));
  await peer.outgoing([1]).write(Uint8Array.of(1, 7, 0));
  inbound.close();
  const ty = [t.option(t.stream(t.u8)), t.stream(t.u8)];
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
  await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
  await assertPending(results[1]);
  assert.match(String(await aborted), /unexpected data/);
});

for (const [
  declared,
  path,
  ty,
  init,
] of /** @type {[boolean, number[], Type, [number[], Uint8Array][]][]} */ ([
  [true, [0], t.stream(t.u8), [[[0], Uint8Array.of(1, 7, 0)]]],
  [false, [0, 0], t.future(t.tuple(t.future(t.u8))), []],
])) {
  test(
    `data for a closed sub-stream ${declared ? "" : "not yet declared "}fails it`,
    { timeout: 1000 },
    async () => {
      const { inbound, peer, transport, aborted } = feed();
      await peer.outgoing([]).write(Uint8Array.of(0));
      for (const [p, bytes] of init) await peer.outgoing(p).write(bytes);
      await peer.outgoing(path).close();
      await peer.outgoing(path).write(Uint8Array.of(7));
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

test("empty frames for a closed sub-stream are discarded", { timeout: 1000 }, async () => {
  for (const declared of [true, false]) {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    if (declared) await peer.outgoing([0]).write(Uint8Array.of(1, 7, 0));
    const path = declared ? [0] : [0, 0];
    for (let i = 0; i < 3; i++) await peer.outgoing(path).close();
    if (!declared) await peer.outgoing([0]).write(Uint8Array.of(0));
    inbound.close();
    const ty = declared ? t.stream(t.u8) : t.future(t.tuple(t.future()));
    const { results, done, finished } = await invoke(transport, STREAMS, "get", [], [], [ty]);
    if (declared) assert.deepEqual(await collect(results[0]), [7]);
    else assert.equal(await (await results[0])[0], undefined);
    await Promise.all([done, finished]);
    assert.equal(await outcome(aborted), "ok");
  }
});

for (const [
  name,
  ty,
  init,
  path,
] of /** @type {[string, Type, [number[], number[]][], number[]][]} */ ([
  ["the reply does not carry", t.option(t.stream(t.u8)), [[[], [0]]], [0]],
  [
    "beyond the reply",
    t.future(t.u8),
    [
      [[], [0]],
      [[0], [5]],
    ],
    [1],
  ],
  [
    "nested in a finished future",
    t.future(t.u8),
    [
      [[], [0]],
      [[0], [5]],
    ],
    [0, 0],
  ],
  [
    "of a list",
    t.list(t.future(t.u8)),
    [
      [[], [1, 0]],
      [[0, 0], [5]],
    ],
    [0],
  ],
])) {
  test(`a path ${name} discards empty frames and fails on data`, { timeout: 1000 }, async () => {
    for (const data of [false, true]) {
      const { inbound, peer, transport, aborted } = feed();
      for (const [p, bytes] of init) await peer.outgoing(p).write(Uint8Array.from(bytes));
      await peer.outgoing(path).close();
      await peer.outgoing(path).close();
      if (data) await peer.outgoing(path).write(Uint8Array.of(7));
      inbound.close();
      const { finished } = await invoke(transport, STREAMS, "get", [], [], [ty]);
      if (data) {
        await assert.rejects(finished, new RegExp(`unexpected data for sub-stream \\[${path}\\]`));
      } else {
        await finished;
        assert.equal(await outcome(aborted), "ok");
      }
    }
  });
}

test("streams nested in a cancelled stream keep receiving", { timeout: 1000 }, async () => {
  const { inbound, peer, transport } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(1, 0));
  await peer.outgoing([0, 0]).write(Uint8Array.of(1, 7));
  const { results, done, finished } = await invoke(
    transport,
    STREAMS,
    "get",
    [],
    [],
    [t.stream(t.stream(t.u8))],
  );
  let inner;
  for await (const chunk of results[0]) {
    inner = chunk[0];
    break;
  }
  await peer.outgoing([0, 0]).write(Uint8Array.of(1, 8, 0));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  inbound.close();
  assert.deepEqual(await collect(inner), [7, 8]);
  await done;
  await finished;
});

test(
  "a future in a received element of a cancelled stream resolves",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    const { results, done, finished } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.stream(t.future(t.u8))],
    );
    let inner;
    for await (const chunk of results[0]) {
      inner = chunk[0];
      break;
    }
    await peer.outgoing([0, 0]).write(Uint8Array.of(5));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    inbound.close();
    assert.equal(await inner, 5);
    await done;
    await finished;
  },
);

test(
  "data for a value in a chunk a cancelled stream dropped fails the invocation",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0, 1, 0));
    const { results, done } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.stream(t.future(t.u8))],
    );
    const chunks = results[0][Symbol.asyncIterator]();
    const { value } = await chunks.next();
    await tick();
    await chunks.return();
    await peer.outgoing([0, 0]).write(Uint8Array.of(5));
    assert.equal(await value[0], 5);
    await peer.outgoing([0, 1]).write(Uint8Array.of(6));
    inbound.close();
    await assert.rejects(done, /unexpected data for sub-stream \[0,1\]/);
  },
);

test(
  "data pulled for a value in a chunk a cancelled stream dropped fails the invocation",
  { timeout: 1000 },
  async () => {
    for (const [elem, frames] of /** @type {[Type, number[][]][]} */ ([
      [t.future(t.u64), [[0x80]]],
      [t.stream(t.u8), [[3], [1], [2]]],
    ])) {
      const { inbound, peer, transport } = feed();
      await peer.outgoing([]).write(Uint8Array.of(0));
      await peer.outgoing([0]).write(Uint8Array.of(1, 0));
      for (const frame of frames) await peer.outgoing([0, 0]).write(Uint8Array.from(frame));
      const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(elem)]);
      await tick();
      results[0].cancel();
      await peer.outgoing([0]).write(Uint8Array.of(0));
      inbound.close();
      await assert.rejects(done, /unexpected data for sub-stream \[0,0\]/);
    }
  },
);

test(
  "the rest of a frame for a sub-stream discarded while receiving it fails the invocation",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    inbound.push(Uint8Array.of(2, 0, 0, 2, 1));
    const { results } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.stream(t.u8))]);
    await tick();
    results[0].cancel();
    const ok = Symbol("ok");
    assert.equal(
      await Promise.race([
        aborted,
        tick()
          .then(tick)
          .then(() => ok),
      ]),
      ok,
    );
    inbound.push(Uint8Array.of(7));
    assert.match(String(await aborted), /unexpected data for sub-stream \[0,0\]/);
  },
);

test("cancelled stream elements are forgotten", { timeout: 1000 }, async () => {
  const { inbound, peer, transport } = feed();
  inbound.push(Uint8Array.of(0, 1, 0x69, 1, 0x66));
  await peer.outgoing([]).write(Uint8Array.of(0));
  const inv = await accept(transport);
  const { params, done } = await inv.receiveParams([t.stream(t.stream(t.u8))]);
  const consumed = (async () => {
    for await (const chunk of params[0]) for (const s of chunk) s.cancel();
  })();
  for (let i = 0; i < 100; i++) {
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    await peer.outgoing([0, i]).write(Uint8Array.of(0));
    await tick();
    const child = inv.mux.root.children.get(0);
    assert.ok(child);
    assert.ok(child.children.size <= 1);
  }
  await peer.outgoing([0]).write(Uint8Array.of(0));
  inbound.close();
  await consumed;
  await done;
  assert.equal(inv.mux.root.children.size, 0);
});

test(
  "a stream nested in a chunk a cancelled stream dropped stops waiting for room",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    for (let i = 0; i < 129; i++) await peer.outgoing([0, 0]).write(Uint8Array.of(1, i));
    const { results, done } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.stream(t.stream(t.u8))],
    );
    for (let i = 0; i < 20; i++) await tick();
    results[0].cancel();
    await peer.outgoing([0]).write(Uint8Array.of(0));
    inbound.close();
    await done;
  },
);

test("data left unread by the reader of a discarded sub-stream fails it once finished", () => {
  const mux = new Mux({ read: () => new Promise(() => {}), write: () => {}, closeWrite: () => {} });
  mux.seal([], [{ path: [0], nested: true }]);
  mux.seal([0, 0], [{ path: [], nested: false }]);
  const ar = mux.incoming([0, 0]);
  ar.buf = Uint8Array.of(0, 7);
  ar.consume(1);
  mux.discard([0], 0, 1);
  assert.ok(!mux.aborted);
  mux.finish([0, 0], ar);
  assert.match(String(mux.writeError), /unexpected data for sub-stream \[0,0\]/);
});

/**
 * Whether `aborted` settles within a few ticks, with what.
 * @param {Promise<unknown>} aborted @returns {Promise<string>}
 */
function outcome(aborted) {
  return Promise.race([
    aborted.then(String),
    tick()
      .then(tick)
      .then(() => "ok"),
  ]);
}

test(
  "empty frames on paths nested in a closed and finished sub-stream are accepted",
  { timeout: 1000 },
  async () => {
    for (const [
      ty,
      init,
      closes,
    ] of /** @type {[Type, [number[], Uint8Array][], number[][]][]} */ ([
      [
        t.stream(t.future(t.u8)),
        [
          [[0], Uint8Array.of(1, 0)],
          [[0, 0], Uint8Array.of(5)],
          [[0], Uint8Array.of(0)],
        ],
        [[0], [0, 0], [0, 0]],
      ],
      [
        t.future(t.tuple(t.future(t.u8))),
        [
          [[0], Uint8Array.of(0)],
          [[0, 0], Uint8Array.of(5)],
        ],
        [[0], [0, 0]],
      ],
    ])) {
      const { inbound, peer, transport, aborted } = feed();
      await peer.outgoing([]).write(Uint8Array.of(0));
      for (const [p, bytes] of init) await peer.outgoing(p).write(bytes);
      const { results, done } = await invoke(transport, STREAMS, "get", [], [], [ty]);
      const value = await (ty.kind === "future" ? results[0] : collect(results[0]));
      assert.equal(await value[0], 5);
      await done;
      for (const p of closes) await peer.outgoing(p).close();
      inbound.close();
      assert.equal(await outcome(aborted), "ok");
    }
  },
);

test("a finished sub-stream closed twice is accepted", { timeout: 1000 }, async () => {
  const { inbound, peer, transport, aborted } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(0));
  await peer.outgoing([0, 0]).write(Uint8Array.of(5));
  const ty = [t.future(t.tuple(t.future(t.u8)))];
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], ty);
  const [inner] = await results[0];
  assert.equal(await inner, 5);
  await done;
  await peer.outgoing([0, 0]).close();
  await peer.outgoing([0, 0]).close();
  assert.equal(await outcome(aborted), "ok");
  await peer.outgoing([0, 0]).write(Uint8Array.of(6));
  inbound.close();
  assert.match(await outcome(aborted), /unexpected data for sub-stream \[0,0\]/);
});

test(
  "a finished sub-stream closed twice in a stream element not finished is accepted",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0, 0, 0));
    await peer.outgoing([0, 0, 0]).write(Uint8Array.of(5));
    const ty = [t.stream(t.tuple(t.future(t.u8), t.future(t.u8)))];
    const { results } = await invoke(transport, STREAMS, "get", [], [], ty);
    const [[first]] = /** @type {[Promise<number>, Promise<number>][]} */ (
      await collect(results[0])
    );
    assert.equal(await first, 5);
    await peer.outgoing([0, 0, 0]).close();
    await peer.outgoing([0, 0, 0]).close();
    assert.equal(await outcome(aborted), "ok");
    await peer.outgoing([0, 0, 0]).write(Uint8Array.of(6));
    inbound.close();
    assert.match(await outcome(aborted), /unexpected data for sub-stream \[0,0,0\]/);
  },
);

test("finished sub-streams are forgotten", { timeout: 1000 }, async () => {
  const { inbound, peer, transport, aborted } = feed();
  const n = 100;
  inbound.push(Uint8Array.of(0, 1, 0x69, 1, 0x66));
  await peer.outgoing([]).write(Uint8Array.of(n, ...new Array(n).fill(0)));
  for (let k = 0; k < n; k++) await peer.outgoing([0, k]).write(Uint8Array.of(k));
  const inv = await accept(transport);
  const { params, done } = await inv.receiveParams([t.list(t.future(t.u8))]);
  assert.deepEqual(await Promise.all(params[0]), [...new Array(n).keys()]);
  await done;
  assert.equal(inv.mux.root.children.size, 0);
  for (let k = 0; k < n; k++) {
    await peer.outgoing([0, k]).close();
    await peer.outgoing([0, k]).close();
  }
  await peer.outgoing([0, 0]).write(Uint8Array.of(1));
  inbound.close();
  assert.match(await outcome(aborted), /unexpected data for sub-stream \[0,0\]/);
});

test("`done` rejects once decoding aborted the invocation", { timeout: 1000 }, async () => {
  const { inbound, peer, transport } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  await peer.outgoing([0]).write(Uint8Array.of(1, 7, 0, 9));
  inbound.close();
  const { results, done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  assert.deepEqual(await collect(results[0]), [7]);
  await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
});

test("`done` rejects once aborted while a stream waits for room", { timeout: 1000 }, async () => {
  const { peer, transport } = feed();
  await peer.outgoing([]).write(Uint8Array.of(0));
  for (let i = 0; i < 129; i++) await peer.outgoing([0]).write(Uint8Array.of(1, i));
  const { done } = await invoke(transport, STREAMS, "get", [], [], [t.stream(t.u8)]);
  for (let i = 0; i < 20; i++) await tick();
  await peer.outgoing([1]).write(Uint8Array.of(7));
  await assert.rejects(done, /unexpected data for sub-stream \[1\]/);
});

test(
  "data left unread by the reader of a future discarded once decoded fails the invocation",
  { timeout: 1000 },
  async () => {
    /** @type {Uint8Array[]} */
    const frames = [];
    const peer = new Mux({
      read: async () => null,
      write: (bytes) => void frames.push(bytes),
      closeWrite: () => {},
    });
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(1, 0));
    await peer.outgoing([0, 0]).write(Uint8Array.of(0, 1, 7, 0));
    await peer.outgoing([0]).write(Uint8Array.of(0));
    /** @type {Chan<Chunk>[]} */
    let results;
    const transport = {
      read: async () => {
        if (frames.length === 2) for (let i = 0; i < 10; i++) await tick();
        if (frames.length === 1) results[0].cancel();
        return frames.shift() ?? null;
      },
      write: () => {},
      closeWrite: () => {},
    };
    let done;
    ({ results, done } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.stream(t.future(t.stream(t.u8)))],
    ));
    await assert.rejects(done, /unexpected data for sub-stream \[0,0\]/);
  },
);

test(
  "data buffered for a path declared without taking it fails the invocation",
  { timeout: 1000 },
  async () => {
    const { inbound, peer, transport, aborted } = feed();
    await peer.outgoing([0]).write(Uint8Array.of(7));
    inbound.close();
    const mux = new Mux(transport);
    mux.start();
    await tick();
    mux._declare(mux.root, { path: [0], nested: false }, undefined);
    mux._settle({ at: mux.root, i: 0 });
    assert.match(await outcome(aborted), /unexpected data for sub-stream \[0\]/);
  },
);

test("a failure after `done` rejects `finished`", { timeout: 1000 }, async () => {
  for (const close of [false, true]) {
    const { inbound, peer, transport } = feed();
    await peer.outgoing([]).write(Uint8Array.of(0));
    await peer.outgoing([0]).write(Uint8Array.of(5));
    const { results, done, finished } = await invoke(
      transport,
      STREAMS,
      "get",
      [],
      [],
      [t.future(t.u8)],
    );
    assert.equal(await results[0], 5);
    await done;
    if (close) {
      inbound.close();
      await finished;
      continue;
    }
    const pending = Symbol("pending");
    assert.equal(await Promise.race([finished, tick().then(() => pending)]), pending);
    await peer.outgoing([0]).write(Uint8Array.of(6));
    await assert.rejects(finished, /unexpected data for sub-stream \[0\]/);
  }
});

test("an accepted invocation is finished once both sides end transmit", async () => {
  const [client, server] = duplexPair();
  const serving = (async () => {
    const inv = await accept(server);
    const { params, done } = await inv.receiveParams([t.string]);
    await Promise.all([done, inv.sendResults([t.string], [params[0]])]);
    await inv.finished;
  })();
  const { results, done, finished } = await invoke(
    client,
    STREAMS,
    "echo",
    [t.string],
    ["hi"],
    [t.string],
  );
  assert.equal(results[0], "hi");
  await Promise.all([done, finished, serving]);
});

test("a transport must communicate end of transmit", async () => {
  /** @type {unknown[]} */
  const aborts = [];
  /** @type {Omit<Transport, "closeWrite">} */
  const transport = {
    read: async () => null,
    write: () => {},
    abort: (err) => void aborts.push(err),
  };
  // @ts-expect-error
  assert.throws(() => new Mux(transport), /closeWrite/);
  // @ts-expect-error
  await assert.rejects(invoke(transport, STREAMS, "get", [], [], []), /closeWrite/);
  // @ts-expect-error
  await assert.rejects(accept(transport), /closeWrite/);
  await tick();
  assert.equal(aborts.length, 3);
  for (const err of aborts) assert.match(String(err), /closeWrite/);
});

test("an accepted invocation is finished once its parameters are decoded", async () => {
  for (const truncated of [false, true]) {
    const { inbound, peer, transport } = feed();
    inbound.push(Uint8Array.of(0, 1, 0x69, 1, 0x66));
    await peer.outgoing([]).write(Uint8Array.of(0));
    const n = truncated ? 1 : 130;
    for (let k = 0; k < n; k++) await peer.outgoing([0]).write(Uint8Array.of(1, k));
    if (!truncated) await peer.outgoing([0]).write(Uint8Array.of(0));
    inbound.close();
    const inv = await accept(transport);
    if (truncated) {
      await inv.sendResults([], []);
      await inv.mux.ended;
      const { done } = await inv.receiveParams([t.stream(t.u8)]);
      await assert.rejects(done, /unexpected end of input/);
      await assert.rejects(inv.finished, /unexpected end of input/);
      continue;
    }
    const { params, done } = await inv.receiveParams([t.stream(t.u8)]);
    await inv.sendResults([], []);
    await inv.mux.ended;
    const pending = Symbol("pending");
    assert.equal(
      await Promise.race([
        inv.finished,
        tick()
          .then(tick)
          .then(() => pending),
      ]),
      pending,
    );
    assert.deepEqual(await collect(params[0]), [...new Array(n).keys()]);
    await Promise.all([done, inv.finished]);
  }
});

test("an accepted invocation is not finished until its parameters are received", async () => {
  const { inbound, peer, transport } = feed();
  inbound.push(Uint8Array.of(0, 1, 0x69, 1, 0x66));
  await peer.outgoing([]).write(Uint8Array.of(7));
  inbound.close();
  const inv = await accept(transport);
  await inv.sendResults([], []);
  await inv.mux.ended;
  const pending = Symbol("pending");
  assert.equal(await Promise.race([inv.finished, tick().then(() => pending)]), pending);
  const { params } = await inv.receiveParams([t.u8]);
  assert.deepEqual(params, [7]);
  await inv.finished;
});

test(
  "a value decoded after its element was discarded declares nothing",
  { timeout: 1000 },
  async () => {
    for (const [nested, data] of /** @type {[number[], boolean][]} */ ([
      [[], false],
      [[], true],
      [[0], true],
    ])) {
      const { inbound, peer, transport, aborted } = feed();
      const mux = new Mux(transport);
      mux.seal([], [{ path: [0], nested: true }]);
      assert.ok(mux.seal([0, 0], [{ path: [], nested: true }]));
      mux.discard([0], 0, 1);
      assert.equal(mux.seal([0, 0], [{ path: nested, nested: false }]), false);
      assert.equal(mux._get([0, 0]), undefined);
      const path = [0, 0, ...nested];
      await peer.outgoing(path).close();
      if (data) await peer.outgoing(path).write(Uint8Array.of(1, 7));
      inbound.close();
      mux.start();
      if (data) {
        assert.match(
          await outcome(aborted),
          new RegExp(`unexpected data for sub-stream \\[${path}\\]`),
        );
      } else {
        assert.equal(await outcome(aborted), "ok");
      }
    }
  },
);
