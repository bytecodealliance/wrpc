# `@bytecodealliance/wrpc`

A [wRPC](https://github.com/bytecodealliance/wrpc) client for JavaScript: a
transport-agnostic, component-native implementation of the wRPC wire format and
its mandatory framing. It encodes and decodes [component model] values against a
WIT type tree and invokes (or serves) functions over a single bidirectional
byte stream, multiplexing each invocation's synchronous and **asynchronous**
(`stream` / `future`) parameters and results — leaving the byte transport
(WebTransport, WebSocket, a `MessagePort`, …) up to you.

It is dependency-free and ships as standard ES modules, so it runs directly in
the browser (`<script type="module">`) as well as through a bundler or Node.

[component model]: https://component-model.bytecodealliance.org

## Install

```sh
npm install @bytecodealliance/wrpc
```

## Quick start

Describe the function's parameter and result types with the `t` (types)
builders, then `invoke` it over a transport. A transport is any duplex of byte
chunks; `fromWebStreams` adapts a WHATWG `{ readable, writable }` pair, such as
a WebTransport bidirectional stream:

```js
import { t, invoke, fromWebStreams } from "@bytecodealliance/wrpc";

const store = "wasi:keyvalue/store@0.2.0-draft2";
const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });

// open: func(identifier: string) -> result<bucket, error>
const transport = fromWebStreams(await conn.createBidirectionalStream());
const { results, done } = await invoke(
  transport,
  store,
  "open",
  [t.string],
  [""],
  [t.result(t.own("bucket"), error)],
);

const [opened] = results;
if (opened.tag === "err") throw opened.val;
const bucket = opened.val; // a Uint8Array handle
await done; // all asynchronous I/O has completed
```

`invoke` runs the framing (a single connection per invocation, the protocol
byte and instance/function names, then the root parameter and result frames)
and resolves with the decoded `results` as soon as the synchronous reply
arrives. `stream` results are async iterables of chunks and `future` results
are promises — both fed as their data arrives over the multiplexed
sub-streams — and `done` resolves once every asynchronous parameter and result
has been transmitted.

### Streams and futures

`t.stream(elem)` and `t.future(some)` describe asynchronous values. A `stream`
parameter is given as an (async-)iterable of chunks (a `Uint8Array` for
`stream<u8>`, an array of elements otherwise); a `future` as a value, a promise,
or a thunk. Decoded `stream` results are async iterables of chunks and `future`
results are promises:

```js
import { t, invoke, fromWebStreams } from "@bytecodealliance/wrpc";

// echo: func(r: req) -> tuple<stream<u64>, stream<u8>>
const req = t.record({ numbers: t.stream(t.u64), bytes: t.stream(t.u8) });
const { results, done } = await invoke(
  fromWebStreams(stream),
  "wrpc-examples:streams/handler",
  "echo",
  [req],
  [{ numbers: [[1n, 2n, 3n]], bytes: new Uint8Array([1, 2, 3]) }],
  [t.tuple(t.stream(t.u64), t.stream(t.u8))],
);

const [numbers, bytes] = results[0];
for await (const chunk of numbers) console.log(chunk); // [1n, 2n, 3n]
for await (const chunk of bytes) console.log(chunk); // Uint8Array([1, 2, 3])
await done;
```

### Serving

`accept` is the server-side counterpart: read an invocation's header off a
transport, decode its parameters, and send the results.

```js
import { t, accept } from "@bytecodealliance/wrpc";

const inv = await accept(transport); // { instance, func, ... }
const { params, done: rx } = await inv.receiveParams([t.string]);
await Promise.all([rx, inv.sendResults([t.result(t.own("bucket"), error)], [{ tag: "ok", val: handle }])]);
```

## Value mapping

Component-model values map to JavaScript following the same conventions as
[`jco`](https://github.com/bytecodealliance/jco):

| WIT                       | JavaScript                                   |
| ------------------------- | -------------------------------------------- |
| `bool`                    | `boolean`                                    |
| `s8`–`s32`, `u8`–`u32`    | `number`                                     |
| `s64`, `u64`              | `bigint`                                     |
| `f32`, `f64`              | `number`                                     |
| `char`, `string`          | `string`                                     |
| `list<u8>`                | `Uint8Array`                                 |
| `list<T>`                 | `Array`                                      |
| `tuple<...>`              | `Array`                                      |
| `record`                  | object keyed by field name                   |
| `option<T>`               | `T \| undefined`                             |
| `result<O, E>`            | `{ tag: "ok", val } \| { tag: "err", val }`  |
| `variant`                 | `{ tag: caseName, val? }`                    |
| `enum`                    | `string` (the case name)                     |
| `flags`                   | `{ [name]: boolean }`                        |
| `future<T>`               | `Promise<T>` (or, when sending, a thunk)     |
| `stream<T>`               | async iterable of chunks (`Uint8Array` for `stream<u8>`) |
| `own<R>` / `borrow<R>`    | `Uint8Array` (the opaque handle bytes)       |

## API

- `t` / `types` — type-tree builders (`bool`, `list`, `record`, `result`, `stream`, `future`, …).
- `invoke(transport, instance, func, paramTypes, args, resultTypes?)` — invoke a function; resolves to `{ results, done }`.
- `accept(transport)` — accept an invocation; resolves to `{ instance, func, receiveParams, sendResults, mux }`.
- `fromWebStreams({ readable, writable })` — adapt a WHATWG stream duplex to a transport.
- `encodeValue(writer, ty, value, sink?, path?)` / `decodeValue(reader, ty, sink?, path?)` — the low-level value codec.
- `Mux` / `Outgoing` — the framing multiplexer, for custom session control.
- `Writer` / `Reader` / `AsyncReader` / `Chan` — byte and async primitives.

## Development

```sh
npm test       # codec / framing / session tests (node --test)
npm run check  # type-check the sources and declarations (tsc)
```

## License

Apache-2.0 WITH LLVM-exception
