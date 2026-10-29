# `@bytecodealliance/wrpc`

A [wRPC](https://github.com/bytecodealliance/wrpc) client and server for JavaScript: a
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
chunks that can communicate end of transmit (see [Transports](#transports));
`fromWebStreams` adapts a WHATWG `{ readable, writable }` pair, such as a
WebTransport bidirectional stream:

```js
import { t, invoke, fromWebStreams } from "@bytecodealliance/wrpc";

const store = "wasi:keyvalue/store@0.2.0-draft2";
const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });

// open: func(identifier: string) -> result<bucket, error>
const transport = fromWebStreams(await conn.createBidirectionalStream());
const { results, done, finished } = await invoke(
  transport,
  store,
  "open",
  [t.string],
  [""],
  [t.result(t.own("bucket"), error)],
);

const [opened] = results;
if (opened.kind === "error") throw opened.value;
const bucket = opened.value; // a Uint8Array handle
await done; // all asynchronous I/O has completed
await finished; // the server ended transmit too
```

`invoke` runs the framing (a single connection per invocation, the protocol
byte and instance/function names, then the root parameter and result frames)
and resolves with the decoded `results` as soon as the synchronous reply
arrives. `stream` results are async iterables of chunks and `future` results
are promises — both fed as their data arrives over the multiplexed
sub-streams — and `done` resolves once every asynchronous parameter and result
has been transmitted. If the arguments do not match their types, or any of
that I/O fails, the invocation fails and the transport is aborted.

`finished` resolves once, in addition, the peer has ended transmit and every
frame it sent has been received and decoded, and rejects if the invocation
fails before then, including after `done`: for example, when the peer sends
data for a sub-stream that has already ended.

### Streams and futures

`t.stream(elem)` and `t.future(some)` describe asynchronous values, and
`t.future()` a bare `future` without a payload. A `stream` parameter is given as
an (async-)iterable of chunks, each a `list<elem>` value (see
[Value mapping](#value-mapping)), or, for a stream of a numeric type, as a
single typed array of it; a `future` as a value, a promise, or a thunk. Decoded
`stream` results are async iterables of chunks and `future` results are
promises, resolving to `undefined` for a bare `future`:

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
await Promise.all([
  (async () => {
    for await (const chunk of numbers) console.log(chunk); // BigUint64Array([1n, 2n, 3n])
  })(),
  (async () => {
    for await (const chunk of bytes) console.log(chunk); // Uint8Array([1, 2, 3])
  })(),
  done,
]);
```

Received data is buffered only up to a bound per stream: once a stream's
buffer is full, reading from the transport pauses until that stream is
consumed, as in the Rust implementation. So consume streams concurrently with
each other, with awaiting futures, and with `done`: awaiting a future while an
earlier stream is left unread can wait forever, since the future's data may be
queued behind the stream's on the transport. Leave a `for await` loop early (or call
`cancel()`) to stop receiving a stream you no longer need. As in the Rust
implementation, any further chunk of it then fails the invocation, while its
end does not. The streams and futures nested in the chunks you received keep
receiving, while those nested in the chunks it buffered, which are dropped, are
discarded: any data for them fails the invocation.

Data the peer sends for a sub-stream that the decoded values do not carry —
for a `stream` in an `option` that is `none`, say — or for one it already
closed with an empty frame fails the invocation, while such empty frames are
discarded.

Received values never fail: if receiving a `future` or `stream` fails, it stays
pending forever. The failure is reported by `done`, as in the Rust
implementation.

### Serving

`accept` is the server-side counterpart: read an invocation's header off a
transport, decode its parameters, and send the results.

```js
import { t, accept } from "@bytecodealliance/wrpc";

const inv = await accept(transport); // { instance, func, ... }
const { params, done: rx } = await inv.receiveParams([t.string]);
await Promise.all([
  rx,
  inv.sendResults([t.result(t.own("bucket"), error)], [{ kind: "ok", value: handle }]),
]);
await inv.finished; // the client ended transmit, its data was decoded and the results were sent
```

### Transports

A transport is `{ read, write, closeWrite, abort? }`: `read()` resolves to the
next inbound chunk, or `null` once the peer has ended transmit; `write(bytes)`
sends an outbound chunk; `closeWrite()` communicates end of transmit to the
peer while still reading; and `abort(reason)`, if given, tears both directions
down when an invocation fails. Frames do not communicate end of transmit, so
every transport must: `closeWrite` is required, and an invocation fails if
the transport does not implement it. A peer that reads until end of transmit,
such as the Rust implementation, otherwise waits forever.

`fromWebStreams` maps `closeWrite` to closing the writable side, which suits
byte streams with a half-close, such as WebTransport bidirectional streams or
TCP sockets (with half-open connections allowed). A WebSocket has no
half-close, and closing it ends both directions, so a WebSocket transport must
signal end of transmit within the connection. To interoperate with the Rust
`wrpc-websockets` transport, send data as binary messages and end transmit by
sending an empty text message (`ws.send("")`), keeping the connection open;
when reading, treat an empty text message received from the peer as the end
of transmit. The Rust transport fails an invocation whose connection closes
before that empty text message as an unexpected end of input, and rejects
non-empty text messages.

## Value mapping

Component-model values map to JavaScript following the `ToJSValue` and
`ToComponentValue` conversions of the component model's [JS-API draft], which
native WebAssembly component support in browsers implements:

| WIT                      | JavaScript                                                    |
| ------------------------ | ------------------------------------------------------------- |
| `bool`                   | `boolean`                                                     |
| `s8`–`s32`, `u8`–`u32`   | `number`                                                      |
| `s64`, `u64`             | `bigint`                                                      |
| `f32`, `f64`             | `number`                                                      |
| `char`, `string`         | `string`                                                      |
| `list<s8>`, `list<u8>`   | `Int8Array`, `Uint8Array`                                     |
| `list<s16>`, `list<u16>` | `Int16Array`, `Uint16Array`                                   |
| `list<s32>`, `list<u32>` | `Int32Array`, `Uint32Array`                                   |
| `list<s64>`, `list<u64>` | `BigInt64Array`, `BigUint64Array`                             |
| `list<f32>`, `list<f64>` | `Float32Array`, `Float64Array`                                |
| `list<T>`                | `Array`                                                       |
| `tuple<...>`             | `Array`                                                       |
| `record`                 | null-prototype object keyed by the camel case field names     |
| `option<T>`              | `T \| undefined`                                              |
| `option<option<T>>`      | `{ kind: "some", value } \| { kind: "none" }`                 |
| `result<O, E>`           | `{ kind: "ok", value? } \| { kind: "error", value? }`         |
| `variant`                | `{ kind: caseName, value? }`                                  |
| `enum`                   | `string` (the case name)                                      |
| `flags`                  | null-prototype object of `boolean`s keyed by camel case names |
| `future<T>`              | `Promise<T>` (or, when sending, a thunk)                      |
| `stream<T>`              | async iterable of chunks, each mapped like a `list<T>`        |
| `own<R>` / `borrow<R>`   | `Uint8Array` (the opaque handle bytes)                        |

`option`, `result` and `variant` objects have a null prototype too, and carry
a `value` only if their case has a payload. `t.record` and `t.flags` take the
WIT names of the fields and flags, which are converted to camel case like the
draft does: the first word is lowercased, and the other words are capitalized,
except acronyms, which are kept as they are (`favorite-color` →
`favoriteColor`, `HTTP-error` → `httpError`, `inner-HTML` → `innerHTML`).
Variant case and enum names are kept as they are in WIT.

When sending, values are converted like the draft does, too:

- Integers are converted to numbers and wrap, with `NaN` and infinities
  becoming `0`, while `s64` and `u64` require a `bigint` (or a `boolean` or a
  `string`), which wraps too. Floats are converted to numbers, `bool`s are
  truthiness, and `char`s, `string`s and enums are converted to strings, with
  lone surrogates in `string`s replaced by U+FFFD.
- A `list` or `tuple` is any iterable, whose elements are converted like the
  typed array constructors convert them for lists of numeric types; a
  `tuple`'s missing elements are `undefined`. An `ArrayBuffer` is not
  iterable, so it is not a byte list.
- Record fields and flags are read as properties, also from the prototype, so
  a missing field is `undefined`, and flags are their truthiness.
- `null` is also `none`, but only for an `option` not nested in another one.

It deviates from the draft in that:

- A `result` is always a tagged object, while the draft returns the `ok` value
  of a function's `result` and throws a `WebAssembly.ComponentError` for its
  `error` value.
- Resources are their opaque handle bytes, which are sent like a `list<u8>`,
  while the draft represents them as class instances.
- `future` and `stream` values, which the draft does not specify yet, follow
  the wRPC session API described above, so that a `stream` is received in
  chunks, as sent.

[JS-API draft]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/JS-Explainer.md

## API

- `t` / `types` — type-tree builders (`bool`, `list`, `record`, `result`, `stream`, `future`, …).
- `invoke(transport, instance, func, paramTypes, args, resultTypes?)` — invoke a function; resolves to `{ results, done, finished }`.
- `accept(transport)` — accept an invocation; resolves to `{ instance, func, receiveParams, sendResults, finished, mux }`.
- `fromWebStreams({ readable, writable })` — adapt a WHATWG stream duplex to a transport
  (see [Transports](#transports)).
- `encodeValue(writer, ty, value, sink?, path?)` / `decodeValue(reader, ty, sink?, path?)` — the low-level value codec.
- `decodeSteps(reader, ty, sink?, path?)` / `atomic(reader, f)` — the resumable form of the decoder, driven by `AsyncReader.decode` across chunk boundaries.
- `Mux` / `Outgoing` — the framing multiplexer, for custom session control; `DISCARDED` is the error the reader of a discarded sub-stream fails with.
- `Writer` / `Reader` / `AsyncReader` / `Chan` — byte and async primitives.

## Development

```sh
npm test       # codec / framing / session tests (node --test)
npm run check  # type-check the sources and declarations (tsc)
npm run lint   # lint all JavaScript in the repository (eslint)
npm run fmt    # format all JavaScript in the repository (prettier); `fmt:check` verifies
```

Interoperability with the Rust implementation is tested from the repository
root with `cargo test --test js`.

## License

Apache-2.0 WITH LLVM-exception
