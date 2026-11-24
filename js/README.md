# `@bytecodealliance/wrpc`

> **Warning:** this package is not ready for production use.

A [wRPC](https://github.com/bytecodealliance/wrpc) client and server for JavaScript: a
transport-agnostic, component-native implementation of the wRPC wire format and
its mandatory framing. It encodes and decodes [component model] values against a
WIT type tree and invokes (or serves) functions over a pair of byte streams,
framing each invocation's synchronous and **asynchronous** (`stream` / `future`)
parameters and results — leaving the byte transport (TCP, WebTransport, HTTP, …)
up to you.

It is dependency-free and ships as standard ES modules, so it runs directly in
the browser (`<script type="module">`) as well as through a bundler or Node.

[component model]: https://component-model.bytecodealliance.org

## Install

```sh
npm install @bytecodealliance/wrpc
```

## Quick start

Describe the function's parameter and result types with the `t` (types)
builders, then `invoke` it using a client. A client is anything implementing
`invoke(codec, instance, name, params)`: `Framed`, the frames over a WHATWG
byte stream pair, writable first, such as a WebTransport bidirectional stream,
or the HTTP client of `fromFetch`:

```js
import { Framed, t, invoke } from "@bytecodealliance/wrpc";

const store = "wasi:keyvalue/store@0.2.0-draft2";
const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });

// open: func(identifier: string) -> result<bucket, error>
const { writable, readable } = await conn.createBidirectionalStream();
const clt = new Framed(writable, readable);
const [[opened], io] = await invoke(
  clt,
  store,
  "open",
  [t.string],
  [""],
  [t.result(t.own("bucket"), error)],
);
await io; // the pending parameters were transmitted and the pending results received
await clt.tx.writer.close();

if (opened.kind === "error") throw opened.value;
const bucket = opened.value; // a Uint8Array handle
```

`invoke` encodes the parameters before anything is written, sends the
invocation header and the parameters, and resolves to the decoded results as
soon as they arrive, along with `io`, the promise of the rest of the
invocation, which resolves once every pending parameter is transmitted and
every pending result received, and rejects if any of that fails. Invocations
over one `Framed` run one after another: start the next once `io` resolved.
wRPC never closes or tears down the byte streams: their owner does, through
`clt.tx.writer` and `clt.rx.reader` (see [Transports](#transports)), and must
tear them down once an invocation failed.

### Streams and futures

`t.stream(elem)` and `t.future(some)` describe asynchronous values, `t.future()`
a bare `future` without a payload and `t.stream()` a bare `stream`, whose
chunks are numbers of elements. A `stream` to send is an iterable or async
iterable of chunks, each a `list<elem>` value (see
[Value mapping](#value-mapping)), or, for a stream of a numeric type, a single
typed array of it; a `future` to send is a value, a promise, or a thunk, which
may return a promise. Received `stream` values are `ReadableStream`s of chunks
and `future` values promises, resolving to `undefined` for a bare `future`:

```js
import { t, invoke } from "@bytecodealliance/wrpc";

// echo: func(r: req) -> tuple<stream<u64>, stream<u8>>
const req = t.record({ numbers: t.stream(t.u64), bytes: t.stream(t.u8) });
const [[[numbers, bytes]], io] = await invoke(
  clt,
  "wrpc-examples:streams/handler",
  "echo",
  [req],
  [{ numbers: [[1n, 2n, 3n]], bytes: new Uint8Array([1, 2, 3]) }],
  [t.tuple(t.stream(t.u64), t.stream(t.u8))],
);

await Promise.all([
  (async () => {
    for await (const chunk of numbers) console.log(chunk); // BigUint64Array([1n, 2n, 3n])
  })(),
  (async () => {
    for await (const chunk of bytes) console.log(chunk); // Uint8Array([1, 2, 3])
  })(),
  io,
]);
```

Received data is not buffered, as in the Rust implementation: a pending
`stream` holds at most one chunk not read yet, and until it is read, nothing
else of the invocation is received. So consume the pending values of an
invocation concurrently with each other and with `io`: a stream read only after
another one, or a future awaited while a stream is left unread, can wait
forever, since its data may be queued behind the other stream's. This includes
the values nested in the chunks of a stream: awaiting the futures of a chunk
before reading the next one works, since that one chunk can wait unread, but
awaiting those of several chunks does not. Such a stall is not an error: only a timeout ends
it.

Cancel a `ReadableStream` (or leave a `for await` loop over it early) to stop
receiving it: any further chunk of it then fails the invocation with a
`ReceiveError` of kind `unread`, while its end does not. A stream or future that is merely left
unreferenced cannot be detected, so it stalls the invocation instead.

Any frame the peer sends for a path that the received values do not carry —
for a `stream` in an `option` that is `none`, say — or for one it already
closed, and data after the end of a value fail the invocation at once.

Received values never fail: if receiving a `future` or `stream` fails, it stays
pending forever. The failure is reported once, by `io`, as in the Rust
implementation.

### Serving

A `Listener` accepts the invocations sent over a byte stream pair one after
another: `accept()` resolves to the header of the next one and the `Framed` to
serve it over, or `null` once the client ended its byte stream. `serve` serves
an invocation by calling a function with its parameters and transmitting the
results it returns, resolving once all its I/O is done:

```js
import { Listener, t, serve } from "@bytecodealliance/wrpc";

const lis = new Listener(writable, readable);
try {
  for (let inv; (inv = await lis.accept());) {
    const [{ instance, name }, io] = inv;
    if (instance !== store || name !== "open") throw new Error(`unknown \`${instance}#${name}\``);
    await serve(io, [t.string], [t.result(t.own("bucket"), error)], (identifier) => [
      { kind: "ok", value: new TextEncoder().encode(identifier) },
    ]);
  }
  await lis.io.tx.writer.close();
} catch (err) {
  await lis.io.tx.writer.abort(err);
  await lis.io.rx.reader.cancel(err);
}
```

`serve` rejects with a `ServeError`, whose `kind` names the side at fault:
receiving the parameters or the rest of the invocation failed (`request`), the
function cancelled a pending parameter, for which data was received
(`unread`), the function failed (`handler`), or encoding or transmitting the
results failed (`response`). Nothing is transmitted if the function fails.

### Errors

Errors are classes with a `kind` naming what failed, keeping the error they are
caused by as their `cause`, as in the Rust implementation:

- `InvokeError`: encoding or transmitting the parameters or obtaining the
  response, such as `fetch` rejecting, failed (`request`), a pending result
  was cancelled while data was received for it (`unread`), or receiving the
  results or the rest of the invocation failed (`response`, caused by a
  `ReceiveError`).
- `ServeError`: see [Serving](#serving); `request` is caused by a `ReceiveError`.
- `ReceiveError`: data for a cancelled consumer (`unread`), a frame on a path,
  which is not pending (`unknownPath`), data after the end of a value
  (`dataAfterEnd`), frames or a path ending before its value was complete
  (`truncated`), invalid data (`decode`), or reading the frames failing
  (`frame`, caused by a `FrameDecodeError`).
- `FrameDecodeError`: a path longer than its maximum (`pathTooDeep`, with
  `depth` and `max`), a path or length, which does not fit in a `u32`
  (`overflow`), the byte stream ending inside a frame (`truncated`), or reading
  it failing (`io`).
- `HeaderDecodeError`, of `Listener.accept()` and `accept(request)`: an
  unsupported protocol `version` (`unsupportedVersion`), a name longer than its
  maximum (`nameTooLong`, with `len` and `max`), a name, which is not UTF-8
  (`invalidName`), a name length, which does not fit in a `u32` (`overflow`),
  the byte stream ending inside a header (`truncated`), or reading it failing
  (`io`).

### Transports

Transports provide WHATWG byte streams, which their owner ends or tears down:
closing the writer ends the byte stream the peer reads; aborting it, or
cancelling the reader, tears the connection down. A client ends its writable
once it has no more invocations to send, a server once `accept()` resolved to
`null`. Close a stream that should not be reset, such as that of a
WebTransport bidirectional stream, with `close()` and `cancel()`, and tear it
down with `abort()`. A Node.js TCP socket (with half-open connections allowed)
adapted with `Duplex.toWeb` may be closed cleanly by `abort()`, so reset it
instead:

```js
const { readable, writable } = Duplex.toWeb(socket);
const clt = new Framed(writable, readable);
try {
  // ...
} catch (err) {
  socket.resetAndDestroy();
}
```

`fromFetch(input, options?)` returns an HTTP client, which invokes each
function with a `POST` request to `input` with `options.fetch`
(`globalThis.fetch` by default), reading the response body, whatever the
response status, and tears both bodies down if the invocation fails. The other
`options`, such as `headers` and `signal`, are passed to `fetch`; the
`content-type` defaults to `MEDIA_TYPE`
(`application/vnd.bytecodealliance.wrpc`). To act on the response status or
headers, wrap `fetch` and reject to fail the invocation with an `InvokeError`
of kind `request` caused by the rejection:

```js
const clt = fromFetch(url, {
  fetch: async (input, init) => {
    const res = await fetch(input, init);
    if (!res.ok) throw new Error(`HTTP request failed with status ${res.status}`);
    return res;
  },
});
```

The request body is streamed, which needs a runtime that supports streaming
request bodies: Firefox and Safari do not, and Chromium only does over HTTP/2
and only exposes the response once the request body is complete. With
`buffer: true`, the whole request body is kept in memory and only sent once
the parameters are transmitted, so the server receives nothing until every
pending parameter is complete.

`accept(request)` reads the invocation header of a WHATWG `Request`, such as
in Deno, Bun or a service worker, resolving to it and the `IncomingRequest` to
serve it over, which `serve` resolves to the `Response` to reply with, with
`content-type` set to `MEDIA_TYPE`, once the results are encoded. Its body
transmits the pending results while the rest of the request is received; a
failure after that tears both down. Data following the invocation in a request
or response body fails it.

```js
import { accept } from "@bytecodealliance/wrpc";
import { handle } from "./server/server.js";

Deno.serve(async (request) => {
  const [{ instance, name }, srv] = await accept(request);
  return (
    handle([{ hello: () => "hello from JavaScript" }], instance, name, srv) ??
    new Response(null, { status: 404 })
  );
});
```

## Generating bindings

`wit-bindgen-wrpc js` generates bindings for a WIT world: an ES module and its
TypeScript declarations per interface, importing only this package. This
package ships `wit-bindgen-wrpc` compiled to WebAssembly, so `npx` runs it
without any other installation. For example, for the `client` and `server`
worlds of `examples/wit/hello`:

```sh
npx -p @bytecodealliance/wrpc wit-bindgen-wrpc js --world client --out-dir client examples/wit/hello
npx -p @bytecodealliance/wrpc wit-bindgen-wrpc js --world server --out-dir server examples/wit/hello
```

The module of an imported interface, at `<namespace>/<package>/<interface>.js`,
exports a function per function of it, which takes the client to invoke it
with and resolves to its result once the invocation is done, and the type tree
of each of its types. The functions of a resource are grouped in an object
named after it, with `new` for its constructor. A function with `stream` or
`future` parameters or results and a result resolves to `[result, io]` instead,
like `invoke` does:

```js
import * as handler from "./client/wrpc-examples/hello/handler.js";

console.log(await handler.hello(clt));
```

The world module's `handle(handlers, instance, name, srv)` serves an invocation
of the world's exports using `srv`, such as the `Framed` a `Listener` accepted
or an `IncomingRequest`, calling the corresponding method of its handler with
the parameters and transmitting what it returns, and resolves like `serve`
does, or returns `undefined` if the world does not export the function, so
worlds chain with `??`. `handlers` holds a handler for the functions the world
exports itself, if any, followed by one per exported interface with functions,
in the order of the world (`exports/<namespace>/<package>/<interface>.js`
declares its `Handler` and exports its own `handle(handler, name, srv)`). A
handler returns a `future` result as a promise, or as a thunk, which may also
be returned by an `async` handler. The world module's `served` lists the
`[instance, function]` pairs `handle` handles:

```js
import { handle } from "./server/server.js";

for (let inv; (inv = await lis.accept());) {
  const [{ instance, name }, io] = inv;
  await (handle([{ hello: () => "hello from JavaScript" }], instance, name, io) ??
    Promise.reject(new Error(`\`${instance}#${name}\` is not exported`)));
}
```

Names follow the [JS-API draft], so functions are camel case and types are
Pascal case.

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
| `list<T, N>`             | like `list<T>`, with exactly `N` elements                     |
| `map<K, V>`              | `Map`                                                         |
| `tuple<...>`             | `Array`                                                       |
| `record`                 | null-prototype object keyed by the camel case field names     |
| `option<T>`              | `T \| undefined`                                              |
| `option<option<T>>`      | `{ kind: "some", value } \| { kind: "none" }`                 |
| `result<O, E>`           | `{ kind: "ok", value? } \| { kind: "error", value? }`         |
| `variant`                | `{ kind: caseName, value? }`                                  |
| `enum`                   | `string` (the case name)                                      |
| `flags`                  | null-prototype object of `boolean`s keyed by camel case names |
| `future<T>`              | `Promise<T>` (or, when sending, a value or a thunk)           |
| `stream<T>`              | `ReadableStream` of chunks, each mapped like a `list<T>`      |
| `stream`                 | `ReadableStream` of numbers of elements                       |
| `own<R>` / `borrow<R>`   | `Uint8Array` (the opaque handle bytes)                        |

`option`, `result` and `variant` objects have a null prototype too, and carry
a `value` only if their case has a payload. `t.record` and `t.flags` take the
WIT names of the fields and flags, which are converted to camel case like the
draft does: the first word is lowercased, and the other words are capitalized,
except acronyms, which are kept as they are (`favorite-color` →
`favoriteColor`, `HTTP-error` → `httpError`, `inner-HTML` → `innerHTML`).
Variant case and enum names are kept as they are in WIT.

When sending, values are converted like the draft does, too:

- Floats are converted to numbers, `bool`s are truthiness, and `char`s,
  `string`s and enums are converted to strings, with lone surrogates in
  `string`s replaced by U+FFFD.
- A `list` or `tuple` is any iterable, whose elements are converted like the
  typed array constructors convert them for lists of floats; a `tuple`'s
  missing elements are `undefined`. An `ArrayBuffer` is not
  iterable, so it is not a byte list.
- A `map` must be a `Map`.
- Record fields and flags are read as properties, also from the prototype, so
  a missing field is `undefined`, and flags are their truthiness.
- `null` is also `none`, but only for an `option` not nested in another one.

It deviates from the draft in that:

- Integers, also in `list`s, are not converted and do not wrap: sending one
  throws a `TypeError` unless it is a `number`, or a `bigint` for `s64` and
  `u64`, and a `RangeError` unless it is an integer in the type's range.
- A `result` is always a tagged object, while the draft returns the `ok` value
  of a function's `result` and throws a `WebAssembly.ComponentError` for its
  `error` value.
- Resources are their opaque handle bytes, which are sent like a `list<u8>`,
  while the draft represents them as class instances.
- `future` and `stream` values, which the draft does not specify yet, follow
  the wRPC semantics described above, so that a `stream` is received in
  chunks, as sent. A `stream` to send must be an iterable or async iterable:
  `null` is not an empty stream.

[JS-API draft]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/JS-Explainer.md

## API

- `t` / `types` — type-tree builders (`bool`, `list`, `record`, `result`, `stream`, `future`, …).
- `invoke(clt, instance, name, params, values, results)` / `serve(srv, params, results, f)` —
  invoke or serve a function by its parameter and result types.
- `InvokeError` / `ServeError` / `ReceiveError` / `FrameDecodeError` / `HeaderDecodeError` — the
  errors of invoking, serving, receiving values and decoding frames and headers, which callers
  match by class and `kind` (see [Errors](#errors)).
- `Framed` / `Listener` — the frames over a byte stream pair, which invoke functions one after
  another or serve an invocation, and the listener accepting the invocations of a client.
- `fromFetch(input, options?)` / `accept(request)` / `IncomingRequest` — the HTTP client and
  server (see [Transports](#transports)); `MEDIA_TYPE` is their default `content-type`.
- `encoder(ty)` / `decoder(ty)` — the codec tree of a type; `Framed.invoke(codec, ...)` and
  `Framed.serve(codec, f)` take a `{ enc, dec }` pair of them.
- `transmit(enc, tx, value)` / `receive(dec, rx)` — send or receive a value and its pending
  values over frames.
- `pack(enc, value, writer)` / `unpack(dec, reader)` — encode or decode a value without pending
  values.
- `FrameWriter` / `FrameReader` / `PROTOCOL` — the low-level framing, for direct integrations.
- `Writer` / `Reader` / `atomic(reader, f)` — byte primitives and the resumable decoder step.

## Development

```sh
npm test       # codec / framing / HTTP tests (node --test)
npm run check  # type-check all JavaScript in the repository (tsc)
npm run lint   # lint all JavaScript in the repository (eslint)
npm run fmt    # format all JavaScript in the repository (prettier); `fmt:check` verifies
```

Interoperability with the Rust implementation is tested from the repository
root with `cargo test --test js`.

## License

Apache-2.0 WITH LLVM-exception
