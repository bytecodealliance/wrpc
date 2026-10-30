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
 * @typedef {[number[], number[]]} Frame a path and its data, closing it if empty
 * @typedef {Awaited<ReturnType<typeof feed>>} Feed
 */

const STREAMS = "wrpc-examples:streams/handler";

/** The header of an invocation of `f` on `i`. */
const HEADER = Uint8Array.of(0, 1, 0x69, 1, 0x66);

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
 * A transport fed by the `frames` a peer writes, then by any more it `send`s,
 * until `inbound` is closed, counting its reads.
 * @param {Frame[]} [frames]
 */
async function feed(frames = []) {
  const inbound = new Chan();
  const peer = new Mux({
    read: async () => null,
    write: (bytes) => void inbound.push(bytes),
    closeWrite: () => {},
  });
  /** @param {Frame[]} frames */
  const send = async (frames) => {
    for (const [path, data] of frames) {
      const out = peer.outgoing(path);
      await (data.length ? out.write(Uint8Array.from(data)) : out.close());
    }
  };
  await send(frames);
  /** @type {(reason: unknown) => void} */
  let abort = () => {};
  const aborted = new Promise((resolve) => (abort = resolve));
  const transport = {
    reads: 0,
    read: async () => (transport.reads++, (await inbound.next()).value ?? null),
    write: () => {},
    closeWrite: () => {},
    abort,
  };
  return { inbound, send, transport, aborted };
}

/** @param {Transport} transport @param {Type[]} types */
const get = (transport, types) => invoke(transport, STREAMS, "get", [], [], types);

/**
 * Invoke `echo` with `arg` of type `ty` on a server sending its parameter back.
 * @param {Type} ty @param {InputValue} arg
 */
async function echo(ty, arg) {
  const [client, server] = duplexPair();
  const serving = (async () => {
    const inv = await accept(server);
    assert.deepEqual([inv.instance, inv.func], [STREAMS, "echo"]);
    const { params, done } = await inv.receiveParams([ty]);
    await Promise.all([done, inv.sendResults([ty], params), inv.finished]);
  })();
  const { results, done, finished } = await invoke(client, STREAMS, "echo", [ty], [arg], [ty]);
  return { result: results[0], ended: Promise.all([done, finished, serving]) };
}

/**
 * Drain an async iterable of chunks into a flat array.
 * @param {AsyncIterable<Chunk>} stream
 */
async function collect(stream) {
  /** @type {any[]} */
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

const tick = () => new Promise((resolve) => setImmediate(resolve));

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

test("values echo end to end", async () => {
  for (const [
    ty,
    arg,
    read,
    expected,
  ] of /** @type {[Type, InputValue, (v: any) => unknown, unknown][]} */ ([
    [
      t.record({ numbers: t.stream(t.u64), bytes: t.stream(t.u8) }),
      { numbers: [[1n, 2n], [3n]], bytes: Uint8Array.of(10, 20) },
      async (v) => [await collect(v.numbers), await collect(v.bytes)],
      [
        [1n, 2n, 3n],
        [10, 20],
      ],
    ],
    [
      t.tuple(t.string, t.own("bucket")),
      ["hi", Uint8Array.of(1)],
      (v) => v,
      ["hi", Uint8Array.of(1)],
    ],
    [t.future(t.string), Promise.resolve("hello"), (v) => v, "hello"],
    [t.future(t.future(t.string)), "deep", (v) => v, "deep"],
    [t.future(t.stream(t.u8)), Uint8Array.of(1, 2), async (v) => collect(await v), [1, 2]],
    [
      t.stream(t.future(t.u8)),
      [[Promise.resolve(5), 6]],
      async (v) => Promise.all(await collect(v)),
      [5, 6],
    ],
    [t.list(t.future(t.u8)), [Promise.resolve(5), 6], (v) => Promise.all(v), [5, 6]],
    [
      t.stream(t.stream(t.u8)),
      [[Uint8Array.of(7)], [Uint8Array.of(8)]],
      async (v) => Promise.all((await collect(v)).map(collect)),
      [[7], [8]],
    ],
  ])) {
    const { result, ended } = await echo(ty, arg);
    assert.deepEqual(await read(result), expected);
    await ended;
  }
});

test("numeric stream chunks are typed arrays and empty ones are not sent", async () => {
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
    for (const [src, chunks] of /** @type {[InputValue, TypedArray[]][]} */ ([
      [Typed.from(vals), [Typed.from(vals)]],
      [
        [vals.slice(0, 1), [], Typed.from(vals.slice(1)), []],
        [Typed.from(vals.slice(0, 1)), Typed.from(vals.slice(1))],
      ],
      [runInNewContext(`${Typed.name}.from(vals)`, { vals }), [Typed.from(vals)]],
    ])) {
      const { result, ended } = await echo(t.stream(elem), src);
      const out = [];
      for await (const chunk of result) {
        assert.ok(chunk instanceof Typed, `\`stream<${elem.kind}>\` chunks are \`${Typed.name}\``);
        out.push(chunk);
      }
      assert.deepEqual(out, chunks);
      await ended;
    }
  }
});

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
  const bytes = Buffer.concat(frames);
  const { inbound, transport } = await feed();
  for (let i = 0; i < bytes.length; i += 7) inbound.push(bytes.subarray(i, i + 7));
  inbound.close();
  const { results, done } = await get(transport, [t.string, t.list(t.u8)]);
  assert.deepEqual(results, ["hello", big]);
  await done;
});

test("a failed transport fails the invocation", async () => {
  const transport = {
    read: () => Promise.reject(new Error("connection refused")),
    write: () => {},
    closeWrite: () => Promise.reject(new Error("connection refused")),
  };
  await assert.rejects(get(transport, [t.u8]), /connection refused/);

  const f = await feed([[[], [0]]]);
  f.inbound.close(new Error("connection reset"));
  const { done } = await get(f.transport, [t.stream(t.u8)]);
  await assert.rejects(done, /connection reset/);
});

test("aborting after the peer closed fails the sub-streams declared later", async () => {
  const mux = new Mux({ read: async () => null, write: () => {}, closeWrite: () => {} });
  mux.start();
  await mux.ingress;
  mux.abort(new Error("boom"));
  mux.seal([], [{ path: [0], nested: false }]);
  await assert.rejects(mux.incoming([0]).u8(), /boom/);
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
  await assert.rejects(get(transport, []), /closeWrite/);
  // @ts-expect-error
  await assert.rejects(accept(transport), /closeWrite/);
  await tick();
  assert.equal(aborts.length, 2);
});

test("invocation header names are bounded", async () => {
  const w = new Writer();
  w.u8(0);
  w.name("a".repeat(2000));
  w.name("f");
  for (const [options, ok] of /** @type {[object | undefined, boolean][]} */ ([
    [undefined, false],
    [{ maxInstanceLength: 2000 }, true],
  ])) {
    /** @type {Uint8Array[]} */
    const bytes = [w.finish()];
    const transport = {
      read: async () => bytes.pop() ?? null,
      write: () => {},
      closeWrite: () => {},
    };
    const accepted = accept(transport, options);
    if (ok) assert.equal((await accepted).instance, "a".repeat(2000));
    else await assert.rejects(accepted, /name length of 2000 exceeds maximum of 1024/);
  }
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
  await tick();
  assert.match(String(aborted), /unsupported/);
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
  await tick();
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

test("invalid results fail the invocation", { timeout: 1000 }, async () => {
  for (const [values, error] of /** @type {[unknown[], RegExp][]} */ ([
    [[], /expected 1 result/],
    [[Symbol()], /Symbol/],
  ])) {
    const [client, server] = duplexPair();
    const serving = (async () => {
      const inv = await accept(server);
      await inv.receiveParams([]);
      await assert.rejects(inv.sendResults([t.string], values), error);
    })();
    await assert.rejects(get(client, [t.string]));
    await serving;
  }
});

test("a failed parameter transmission fails the invocation and aborts the transport", async () => {
  async function* failing() {
    yield [1];
    throw new Error("source failed");
  }
  for (const [arg, writes, error] of /** @type {[InputValue, number, RegExp][]} */ ([
    [failing(), Infinity, /source failed/],
    [Uint8Array.of(1), 2, /broken pipe/],
  ])) {
    let n = writes;
    /** @type {unknown} */
    let aborted;
    /** @type {Transport} */
    const transport = {
      read: () => new Promise((resolve) => setTimeout(() => resolve(null), 10)),
      write: () => (n-- > 0 ? undefined : Promise.reject(new Error("broken pipe"))),
      closeWrite: () => {},
      abort: (reason) => void (aborted = reason),
    };
    await assert.rejects(
      invoke(transport, STREAMS, "echo", [t.stream(t.u8)], [arg], [t.u8]),
      error,
    );
    await tick();
    assert.match(String(aborted), error);
  }
});

test("a failed reply stops sending stream parameters", { timeout: 1000 }, async () => {
  /** @type {(value?: unknown) => void} */
  let stop = () => {};
  const stopped = new Promise((resolve) => (stop = resolve));
  async function* numbers() {
    try {
      for (;;) {
        yield [1n];
        await tick();
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

test("the peer stopping the closing web stream completes it only with code 1", async () => {
  for (const code of [1, 0]) {
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
    const { results, done } = await get(transport, [t.u8]);
    assert.deepEqual(results, [42]);
    if (code === 1) await done;
    else await assert.rejects(done, { name: "WebTransportError", streamErrorCode: 0 });
  }
});

test("writing a frame fails once aborted or overflowing a u32", async () => {
  const mux = new Mux({ read: async () => null, write: () => {}, closeWrite: () => {} });
  await assert.rejects(mux.outgoing([2 ** 32]).write(Uint8Array.of(1)), RangeError);
  mux.abort(new Error("boom"));
  await assert.rejects(mux.outgoing([]).write(new Uint8Array(0)), /boom/);
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

/**
 * A transport replaying a reply of one pending `stream<u8>` result of `n`
 * one-byte chunks.
 * @param {number} n
 */
async function streamingReply(n) {
  const { inbound, transport } = await feed([
    [[], [0]],
    ...Array.from({ length: n }, (_, i) => /** @type {Frame} */ ([[0], [1, i & 0xff]])),
    [[0], [0]],
  ]);
  inbound.close();
  return transport;
}

test("an unconsumed stream result stops reading the transport", { timeout: 1000 }, async () => {
  const transport = await streamingReply(1000);
  const { results, done } = await get(transport, [t.stream(t.u8)]);
  for (let i = 0; i < 100; i++) await tick();
  assert.ok(transport.reads < 200, `read ${transport.reads} frames`);
  assert.equal((await collect(results[0])).length, 1000);
  await done;
  assert.equal(transport.reads, 1003);
});

test(
  "a sub-stream buffered ahead of its declaration is bounded once declared",
  { timeout: 1000 },
  async () => {
    const { inbound, transport } = await feed([
      [[], [0]],
      [
        [0, 0],
        [1, 0],
      ],
      [[0], [0]],
      ...Array.from(
        { length: 1000 },
        (_, i) =>
          /** @type {Frame} */ ([
            [0, 0],
            [1, i & 0xff],
          ]),
      ),
    ]);
    inbound.close();
    const { results } = await get(transport, [t.future(t.tuple(t.stream(t.u8)))]);
    const [inner] = await results[0];
    await inner[Symbol.asyncIterator]().next();
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.ok(transport.reads < 500, `read ${transport.reads} frames`);
  },
);

test(
  "data for a sub-stream declared while ingress waits for room is routed",
  { timeout: 1000 },
  async () => {
    const { transport } = await feed([
      ...Array.from({ length: 64 }, (_, k) => /** @type {Frame} */ ([[1, k], [7]])),
      [[0, 0], [42]],
    ]);
    const mux = new Mux(transport);
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

test("data arriving before its sub-streams are declared is kept", { timeout: 1000 }, async () => {
  const { inbound, transport } = await feed([
    [
      [0, 1],
      [1, 8],
    ],
    [[0, 1], [0]],
    [
      [0, 0],
      [1, 7],
    ],
    [[0, 0], [0]],
    [[], [0, 0]],
  ]);
  inbound.close();
  const { results, done } = await get(transport, [t.tuple(t.stream(t.u8), t.stream(t.u8))]);
  assert.deepEqual(await Promise.all(results[0].map(collect)), [[7], [8]]);
  await done;
});

test("data for results which cannot be pending fails at once", { timeout: 1000 }, async () => {
  const { inbound, transport } = await feed([
    [[2], []],
    [[0], [0]],
  ]);
  inbound.close();
  await assert.rejects(
    get(transport, [t.u32, t.stream(t.u8)]),
    /unexpected data for sub-stream \[0\]/,
  );
});

test("a future without a payload resolves at the end of transmit", async () => {
  for (const data of [false, true]) {
    const { inbound, transport } = await feed(
      data
        ? [
            [[], [0]],
            [[0], [0]],
          ]
        : [[[], [0]]],
    );
    inbound.close();
    const { results, done } = await get(transport, [t.future()]);
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

test("a truncated stream fails", { timeout: 1000 }, async () => {
  for (const truncate of /** @type {((f: Feed) => unknown)[]} */ ([
    ({ inbound }) => inbound.push(Uint8Array.of(1, 0, 5, 1, 6)),
    ({ send }) => send([[[0], [1, 5, 0x80]]]),
    ({ send }) =>
      send([
        [[0], [1, 7]],
        [[0], []],
      ]),
  ])) {
    const f = await feed([[[], [0]]]);
    await truncate(f);
    f.inbound.close();
    const { results, done, finished } = await get(f.transport, [t.stream(t.u8)]);
    await assert.rejects(done, /unexpected end of input/);
    await assert.rejects(finished, /unexpected end of input/);
    await assertPending(results[0]);
  }
});

test("frame data beyond the end of a sub-stream fails it", { timeout: 1000 }, async () => {
  for (const [ty, value] of /** @type {[Type, number[]][]} */ ([
    [t.future(t.u8), [5]],
    [t.stream(t.u8), [1, 5, 0]],
  ])) {
    for (const [extra, split] of /** @type {[number, boolean][]} */ ([
      [3, false],
      [3, true],
      [40, true],
    ])) {
      const data = [...value, ...new Array(extra).fill(9)];
      const w = new Writer();
      w.varU(1);
      w.varU(0);
      w.varU(data.length);
      w.bytes(data);
      const bytes = [0, 1, 0, ...w.finish()];
      const { inbound, transport, aborted } = await feed();
      for (const chunk of split ? bytes.map((b) => [b]) : [bytes]) {
        inbound.push(Uint8Array.from(chunk));
      }
      inbound.close();
      const { results, done, finished } = await get(transport, [ty]);
      assert.deepEqual(ty.kind === "future" ? [await results[0]] : await collect(results[0]), [5]);
      const error = /unexpected data for sub-stream \[0\]/;
      assert.match(String(await aborted), error);
      if (!split) await assert.rejects(done, error);
      await assert.rejects(finished, error);
    }
  }
});

test("data for the root after the reply fails it", { timeout: 1000 }, async () => {
  const { inbound, transport } = await feed([
    [[], [0]],
    ...Array.from({ length: 40 }, (_, i) => /** @type {Frame} */ ([[], [i]])),
    [[0], [1, 7]],
    [[0], [0]],
  ]);
  inbound.close();
  const { results, done } = await get(transport, [t.stream(t.u8)]);
  await assert.rejects(done, /unexpected data for sub-stream \[\]/);
  await assertPending(results[0]);
});

for (const [
  name,
  ty,
  before,
  path,
  after,
] of /** @type {[string, Type, Frame[], number[], Frame[]][]} */ ([
  [
    "of a finished stream",
    t.stream(t.u8),
    [
      [[], [0]],
      [[0], [1, 7, 0]],
    ],
    [0],
    [],
  ],
  [
    "closed before it is declared",
    t.future(t.tuple(t.future())),
    [[[], [0]]],
    [0, 0],
    [[[0], [0]]],
  ],
  [
    "not declared by its value",
    t.future(t.tuple(t.u8, t.future(t.u8))),
    [[[], [0]]],
    [0, 0],
    [
      [[0], [5, 0]],
      [[0, 1], [6]],
    ],
  ],
  ["the reply does not carry", t.option(t.stream(t.u8)), [[[], [0]]], [0], []],
  [
    "beyond the reply",
    t.future(t.u8),
    [
      [[], [0]],
      [[0], [5]],
    ],
    [1],
    [],
  ],
  [
    "longer than the types allow",
    t.future(t.u8),
    [
      [[], [0]],
      [[0], [5]],
    ],
    [0, 0],
    [],
  ],
  [
    "of a list",
    t.list(t.future(t.u8)),
    [
      [[], [1, 0]],
      [[0, 0], [5]],
    ],
    [0],
    [],
  ],
  [
    "of a finished future in a stream element",
    t.stream(t.future(t.u8)),
    [
      [[], [0]],
      [[0], [1, 0]],
      [[0, 0], [5]],
    ],
    [0, 0],
    [[[0], [0]]],
  ],
  [
    "of a finished future in a finished future",
    t.future(t.tuple(t.future(t.u8))),
    [
      [[], [0]],
      [[0], [0]],
      [[0, 0], [5]],
    ],
    [0, 0],
    [],
  ],
])) {
  test(`a path ${name} discards empty frames and fails on data`, { timeout: 1000 }, async () => {
    for (const data of [undefined, "before", "after"]) {
      const { inbound, send, transport, aborted } = await feed(before);
      /** @type {Frame[]} */
      const frames = [
        [path, []],
        [path, []],
      ];
      if (data) frames.splice(data === "before" ? 0 : 2, 0, [path, [7]]);
      await send([...frames, ...after]);
      inbound.close();
      const finished = get(transport, [ty]).then((r) => r.finished);
      if (data) {
        await assert.rejects(
          finished,
          new RegExp(
            `unexpected data for (sub-stream \\[${path}\\]|a sub-stream path of length ${path.length})`,
          ),
        );
      } else {
        await finished;
        assert.equal(await outcome(aborted), "ok");
      }
    }
  });
}

test("ready values in stream elements are received", { timeout: 1000 }, async () => {
  const { inbound, transport } = await feed([
    [[], [0]],
    [[0], [2, 1, 5, 1, 6]],
    [[0], [0]],
  ]);
  inbound.close();
  const { results, done } = await get(transport, [t.stream(t.future(t.u8))]);
  const values = /** @type {Promise<number>[]} */ (await collect(results[0]));
  assert.deepEqual(await Promise.all(values), [5, 6]);
  await done;
});

test(
  "values in received elements of a cancelled stream keep receiving, in dropped ones fail",
  { timeout: 1000 },
  async () => {
    for (const [
      elem,
      data,
      read,
      expected,
    ] of /** @type {[Type, number[], (v: any) => unknown, unknown][]} */ ([
      [t.stream(t.u8), [1, 7, 0], collect, [7]],
      [t.future(t.u8), [5], (v) => v, 5],
    ])) {
      for (const dropped of [false, true]) {
        const { inbound, send, transport } = await feed([
          [[], [0]],
          [[0], dropped ? [1, 0, 1, 0] : [1, 0]],
        ]);
        const { results, done } = await get(transport, [t.stream(elem)]);
        const chunks = results[0][Symbol.asyncIterator]();
        const { value } = await chunks.next();
        await tick();
        await chunks.return();
        await send([[[0, 0], data]]);
        assert.deepEqual(await read(value[0]), expected);
        await send([dropped ? [[0, 1], data] : [[0], [0]]]);
        inbound.close();
        if (dropped) await assert.rejects(done, /unexpected data for sub-stream \[0,1\]/);
        else await done;
      }
    }
  },
);

test(
  "a cancelled stream result with data left fails the invocation",
  { timeout: 1000 },
  async () => {
    const transport = await streamingReply(1000);
    const { results, done } = await get(transport, [t.stream(t.u8)]);
    for await (const chunk of results[0]) {
      assert.deepEqual(chunk, Uint8Array.of(0));
      break;
    }
    await assert.rejects(done, /unexpected data for sub-stream \[0\]/);
    assert.deepEqual(await results[0].next(), { value: undefined, done: true });
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
      const { inbound, send, transport } = await feed([
        [[], [0]],
        [[0], [1, 0]],
        ...frames.map((frame) => /** @type {Frame} */ ([[0, 0], frame])),
      ]);
      const { results, done } = await get(transport, [t.stream(elem)]);
      await tick();
      results[0].cancel();
      await send([[[0], [0]]]);
      inbound.close();
      await assert.rejects(done, /unexpected data for sub-stream \[0,0\]/);
    }
  },
);

test(
  "a stream nested in a chunk a cancelled stream dropped stops waiting for room",
  { timeout: 1000 },
  async () => {
    const { inbound, send, transport } = await feed([
      [[], [0]],
      [[0], [1, 0]],
      ...Array.from(
        { length: 129 },
        (_, i) =>
          /** @type {Frame} */ ([
            [0, 0],
            [1, i],
          ]),
      ),
    ]);
    const { results, done } = await get(transport, [t.stream(t.stream(t.u8))]);
    for (let i = 0; i < 20; i++) await tick();
    results[0].cancel();
    await send([[[0], [0]]]);
    inbound.close();
    await done;
  },
);

test("`done` rejects once aborted while a stream waits for room", { timeout: 1000 }, async () => {
  const { send, transport } = await feed([
    [[], [0]],
    ...Array.from({ length: 129 }, (_, i) => /** @type {Frame} */ ([[0], [1, i]])),
  ]);
  const { done } = await get(transport, [t.stream(t.u8)]);
  for (let i = 0; i < 20; i++) await tick();
  await send([[[1], [7]]]);
  await assert.rejects(done, /unexpected data for sub-stream \[1\]/);
});

test("cancelled stream elements are forgotten", { timeout: 1000 }, async () => {
  const { inbound, send, transport } = await feed();
  inbound.push(HEADER);
  await send([[[], [0]]]);
  const inv = await accept(transport);
  const { params, done } = await inv.receiveParams([t.stream(t.stream(t.u8))]);
  const consumed = (async () => {
    for await (const chunk of params[0]) for (const s of chunk) s.cancel();
  })();
  for (let i = 0; i < 100; i++) {
    await send([
      [[0], [1, 0]],
      [[0, i], [0]],
    ]);
    await tick();
    assert.ok((inv.mux.root.children.get(0)?.children.size ?? 0) <= 1);
  }
  await send([[[0], [0]]]);
  inbound.close();
  await Promise.all([consumed, done]);
  assert.equal(inv.mux.root.children.size, 0);
});

test("finished list elements are forgotten", { timeout: 1000 }, async () => {
  const n = 100;
  const { inbound, send, transport, aborted } = await feed();
  inbound.push(HEADER);
  await send([
    [[], [n, ...new Array(n).fill(0)]],
    ...Array.from({ length: n }, (_, k) => /** @type {Frame} */ ([[0, k], [k]])),
  ]);
  const inv = await accept(transport);
  const { params, done } = await inv.receiveParams([t.list(t.future(t.u8))]);
  assert.deepEqual(await Promise.all(params[0]), [...new Array(n).keys()]);
  await done;
  assert.equal(inv.mux.root.children.size, 0);
  await send([[[0, 0], [1]]]);
  inbound.close();
  assert.match(await outcome(aborted), /unexpected data for sub-stream \[0,0\]/);
});

test(
  "values nested in a discarded element are forgotten along with their data",
  { timeout: 1000 },
  async () => {
    for (const [nested, data] of /** @type {[number[], string | undefined][]} */ ([
      [[], undefined],
      [[], "after"],
      [[0], "after"],
      [[0], "before"],
    ])) {
      const path = [0, 0, ...nested];
      const { inbound, send, transport, aborted } = await feed();
      const mux = new Mux(transport);
      mux.seal([], [{ path: [0], nested: true }]);
      assert.ok(mux.seal([0, 0], [{ path: [], nested: true }]));
      mux.start();
      if (data === "before") await send([[path, [1, 7]]]);
      await tick();
      mux.discard([0], 0, 1);
      assert.equal(mux.seal([0, 0], [{ path: nested, nested: false }]), false);
      assert.equal(mux._get([0, 0]), undefined);
      await send([[path, []]]);
      if (data === "after") await send([[path, [1, 7]]]);
      inbound.close();
      assert.match(await outcome(aborted), data ? /unexpected data for sub-stream \[0,0/ : /^ok$/);
    }
  },
);

test("a failure after `done` rejects `finished`", { timeout: 1000 }, async () => {
  const { send, transport } = await feed([
    [[], [0]],
    [[0], [5]],
  ]);
  const { results, done, finished } = await get(transport, [t.future(t.u8)]);
  assert.equal(await results[0], 5);
  await done;
  const pending = Symbol("pending");
  assert.equal(await Promise.race([finished, tick().then(() => pending)]), pending);
  await send([[[0], [6]]]);
  await assert.rejects(finished, /unexpected data for sub-stream \[0\]/);
});

test("an accepted invocation is finished once its parameters are received and decoded", async () => {
  const n = 130;
  const { inbound, send, transport } = await feed();
  inbound.push(HEADER);
  await send([
    [[], [0]],
    ...Array.from({ length: n }, (_, k) => /** @type {Frame} */ ([[0], [1, k]])),
    [[0], [0]],
  ]);
  inbound.close();
  const inv = await accept(transport);
  await inv.sendResults([], []);
  const pending = Symbol("pending");
  const settled = () =>
    Promise.race([
      inv.finished,
      tick()
        .then(tick)
        .then(() => pending),
    ]);
  assert.equal(await settled(), pending);
  const { params, done } = await inv.receiveParams([t.stream(t.u8)]);
  await inv.mux.ended;
  assert.equal(await settled(), pending);
  assert.deepEqual(await collect(params[0]), [...new Array(n).keys()]);
  await Promise.all([done, inv.finished]);
});
