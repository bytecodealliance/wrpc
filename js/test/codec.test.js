import assert from "node:assert/strict";
import { test } from "node:test";

import * as t from "../src/types.js";
import { FrameReader, FrameWriter, Framed, Listener } from "../src/frame.js";
import { UnreadError, decoder, encoder, receive, transmit } from "../src/codec.js";

/**
 * @typedef {import("../src/index.js").Type} Type
 * @typedef {import("../src/index.js").InputValue} InputValue
 */

const STREAM = t.tuple(t.stream(t.u8));

/** @param {number[]} path @param {string} data */
const frame = (path, data) => [path.length, ...path, data.length, ...Buffer.from(data, "latin1")];

/** @param {...(readonly number[])} chunks */
const readable = (...chunks) =>
  new ReadableStream({
    pull(c) {
      const chunk = chunks.shift();
      if (chunk) c.enqueue(Uint8Array.from(chunk));
      else c.close();
    },
  });

/** @param {ReadableStream<any>} s */
async function collect(s) {
  const out = [];
  for await (const chunk of s) {
    if (typeof chunk === "number") out.push(chunk);
    else out.push(...chunk);
  }
  return out;
}

/**
 * Invoke `instance#name` over `io`, as `wrpc-core`'s `invoke` does.
 * @param {Framed} io @param {Type[]} params @param {InputValue[]} values @param {Type[]} results
 * @returns {Promise<[any[], Promise<unknown>]>}
 */
async function invoke(io, params, values, results) {
  await io.tx.header({ instance: "i", name: "f" });
  const tx = transmit(encoder(t.tuple(...params)), io.tx, values);
  const [out, rx] = await Promise.race([
    receive(decoder(t.tuple(...results)), io.rx),
    tx.then(() => new Promise(() => {})),
  ]);
  return [out, Promise.all([tx, rx])];
}

/**
 * Serve the invocation over `io`, as `wrpc-core`'s `serve` does.
 * @param {Framed} io @param {Type[]} params @param {Type[]} results
 * @param {(...params: any[]) => InputValue[] | Promise<InputValue[]>} f
 */
async function serve(io, params, results, f) {
  const [values, rx] = await receive(decoder(t.tuple(...params)), io.rx);
  const tx = (async () => transmit(encoder(t.tuple(...results)), io.tx, await f(...values)))();
  await Promise.all([rx, tx]);
}

test("frames", async () => {
  const bytes = [
    ...[1, 1, 0x69, 1, 0x66],
    ...[3, 0, 1, 2, 4, ...Buffer.from("test")],
    ...[0, 0],
    ...[1, 0x42, 9, ...Buffer.from("\x7fƒ𐍈Ő")],
  ];
  /** @type {number[]} */
  const written = [];
  const tx = new FrameWriter(new WritableStream({ write: (c) => void written.push(...c) }));
  await tx.header({ instance: "i", name: "f" });
  await tx.write([0, 1, 2], new TextEncoder().encode("test"));
  await tx.write([7], new Uint8Array());
  await tx.close([]);
  await tx.write([0x42], new TextEncoder().encode("\x7fƒ𐍈Ő"));
  assert.deepEqual(written, bytes);

  const rx = new FrameReader(readable(...bytes.map((b) => [b])));
  assert.deepEqual(await rx.header(), { instance: "i", name: "f" });
  /** @type {[readonly number[], string][]} */
  const frames = [];
  for (let data; (data = await rx.next());) {
    frames.push([rx.path, Buffer.from(data).toString("latin1")]);
  }
  assert.deepEqual(frames, [
    ...[..."test"].map((c) => [[0, 1, 2], c]),
    [[], ""],
    ...[...Buffer.from("\x7fƒ𐍈Ő")].map((b) => [[0x42], String.fromCharCode(b)]),
  ]);

  for (const [input, msg] of /** @type {const} */ ([
    [[65, ...Array(65).fill(0), 0], /path length of `65` exceeds maximum of `64`/],
    [[1, 0x80], /unexpected end of input/],
  ])) {
    await assert.rejects(new FrameReader(readable(input)).next(), msg);
  }
  for (const [input, msg] of /** @type {const} */ ([
    [[1, 1, 0x69, 2, 0x66, 0x66], /name length of 2 exceeds maximum of 1/],
    [[2], /unsupported version byte: 2/],
  ])) {
    await assert.rejects(new FrameReader(readable(input)).header({ maxNameLength: 1 }), msg);
  }
});

test("receiving reads nothing past the last path closed", async () => {
  const bytes = [
    ...frame([], "\0"),
    ...frame([0], "\x03foo"),
    ...frame([], ""),
    ...frame([0], "\x03b"),
    ...frame([0], "ar"),
    ...frame([0], ""),
    ...[1, 1, 0x69, 1, 0x66],
  ];
  const lis = new Listener(new WritableStream(), readable(bytes));
  const [[s], rx] = await receive(decoder(STREAM), lis.io.rx);
  const [out] = await Promise.all([collect(s), rx]);
  assert.equal(Buffer.from(out).toString(), "foobar");
  assert.deepEqual((await lis.accept())?.[0], { instance: "i", name: "f" });
});

test("invalid frames fail receiving at once", async () => {
  const root = [...frame([], "\0"), ...frame([], "")];
  for (const [bytes, msg] of /** @type {const} */ ([
    [[...root, ...frame([1], "")], /unknown path `\[1\]`/],
    [frame([], "\0\x2a"), /data after value end/],
    [root, /ingress ended before all paths were closed/],
    [[...root, ...frame([0], "\x03fo"), ...frame([0], "")], /stream closed within a chunk/],
    [[...root, 1, 0, 5, 0x2a], /frame truncated/],
    [[...frame([0], "\0")], /unknown path `\[0\]`/],
  ])) {
    const run = async () => {
      const [[s], rx] = await receive(decoder(STREAM), new FrameReader(readable(bytes)));
      void collect(s);
      await rx;
    };
    await assert.rejects(run, msg);
  }
});

test("invocations echo pending values one after another", { timeout: 1000 }, async () => {
  const types = [
    t.stream(t.u8),
    t.stream(t.future(t.u32)),
    t.record({ a: t.future(t.future(t.string)), b: t.future(), c: t.string }),
    t.future(t.stream(t.list(t.string))),
  ];
  const up = new TransformStream();
  const down = new TransformStream();
  const lis = new Listener(down.writable, up.readable);
  const served = (async () => {
    for (let inv; (inv = await lis.accept());) await serve(inv[1], types, types, (...v) => v);
    await lis.io.tx.writer.close();
  })();
  const io = new Framed(up.writable, down.readable);
  for (let i = 0; i < 2; i++) {
    const [[bytes, futs, rec, nested], done] = await invoke(
      io,
      types,
      [
        (async function* () {
          yield Uint8Array.of(1);
          yield [];
          yield [2, 3];
        })(),
        [[1, Promise.resolve(2)], [() => 3 + i]],
        { a: () => () => Promise.resolve("a"), b: Promise.resolve(undefined), c: "c" },
        [[["x"]], [[], ["y", "z"]]],
      ],
      types,
    );
    const [out] = await Promise.all([
      Promise.all([
        collect(bytes),
        collect(futs).then((fs) => Promise.all(fs)),
        rec.a,
        rec.b,
        rec.c,
        nested.then(collect),
      ]),
      done,
    ]);
    assert.deepEqual(out, [[1, 2, 3], [1, 2, 3 + i], "a", undefined, "c", [["x"], [], ["y", "z"]]]);
  }
  await io.tx.writer.close();
  await served;
  assert.equal((await io.rx.reader.read()).done, true);
});

test("invalid values fail before anything is written", async () => {
  /** @type {Uint8Array[]} */
  const written = [];
  const tx = new FrameWriter(new WritableStream({ write: (c) => void written.push(c) }));
  const enc = encoder(t.tuple(t.stream(t.u8), t.u8));
  await assert.rejects(transmit(enc, tx, [[], 256]), RangeError);
  await assert.rejects(transmit(enc, tx, [42, 0]), TypeError);
  assert.deepEqual(written, []);
});

test("pending values nested in every kind echo", { timeout: 1000 }, async () => {
  /** @param {any} f */
  const all = (f) => Promise.all(f);
  const cases = /** @type {[Type, InputValue, (v: any) => unknown, unknown][]} */ ([
    [t.list(t.future(t.u32)), [1, Promise.resolve(2)], all, [1, 2]],
    [
      t.list(t.stream(t.u8), 2),
      [[Uint8Array.of(1)], [Uint8Array.of(2)]],
      (v) => Promise.all(v.map(collect)),
      [[1], [2]],
    ],
    [
      t.map(t.string, t.future(t.string)),
      new Map([["a", "x"]]),
      (v) => Promise.all([...v].map(async ([k, f]) => [k, await f])),
      [["a", "x"]],
    ],
    [t.option(t.future(t.u32)), () => 3, (v) => v, 3],
    [
      t.result(t.stream(t.u8), t.future(t.u32)),
      { kind: "error", value: 4 },
      async (v) => [v.kind, await v.value],
      ["error", 4],
    ],
    [
      t.variant({ a: t.future(t.string), b: null }),
      { kind: "a", value: "v" },
      async (v) => [v.kind, await v.value],
      ["a", "v"],
    ],
    [t.stream(t.option(t.future(t.u8))), [[null, 5]], (v) => collect(v).then(all), [undefined, 5]],
    [t.stream(), [2, 0, 3], (v) => collect(v), [2, 3]],
    [t.own("r"), Uint8Array.of(9), (v) => v, Uint8Array.of(9)],
  ]);
  const types = cases.map(([ty]) => ty);
  const up = new TransformStream();
  const down = new TransformStream();
  const lis = new Listener(down.writable, up.readable);
  void lis.accept().then((inv) => inv && serve(inv[1], types, types, (...v) => v));
  const io = new Framed(up.writable, down.readable);
  const [out, done] = await invoke(
    io,
    types,
    cases.map(([, v]) => v),
    types,
  );
  const [got] = await Promise.all([Promise.all(cases.map(([, , read], i) => read(out[i]))), done]);
  assert.deepEqual(
    got,
    cases.map(([, , , want]) => want),
  );
});

test("an unread stream stalls receiving, a cancelled one fails it", { timeout: 1000 }, async () => {
  const idle = async () => {
    for (let i = 0; i < 10; i++) await new Promise((resolve) => setImmediate(resolve));
  };
  const up = new TransformStream();
  const io = new Framed(up.writable, new ReadableStream());
  const params = [0, 1, 2].map((i) => Buffer.from(`${i}`));
  let sent = false;
  void transmit(encoder(STREAM), io.tx, [params]).then(() => (sent = true));
  const [[s], rx] = await receive(decoder(STREAM), new FrameReader(up.readable));
  let settled = false;
  void rx.catch(() => {}).finally(() => (settled = true));
  const reader = s.getReader();
  await idle();
  assert.equal(Buffer.from((await reader.read()).value).toString(), "0");
  await idle();
  assert.equal(sent, false);
  assert.equal(settled, false);
  await reader.cancel();
  await assert.rejects(rx, UnreadError);
});
