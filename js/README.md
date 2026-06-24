# `@bytecodealliance/wrpc`

The [wRPC](https://github.com/bytecodealliance/wrpc) codec for JavaScript: a
transport-agnostic encoder/decoder for the wRPC wire format. It encodes and
decodes [component model] values against a WIT type tree and frames an
invocation's parameters and results, leaving the byte transport (WebTransport,
WebSocket, a `MessagePort`, …) up to you.

It is dependency-free and ships as standard ES modules, so it runs directly in
the browser (`<script type="module">`) as well as through a bundler or Node.

[component model]: https://component-model.bytecodealliance.org

## Install

```sh
npm install @bytecodealliance/wrpc
```

## Quick start

Describe a value's type with the `t` (types) builders, then encode and decode:

```js
import { t, encode, decode } from "@bytecodealliance/wrpc";

const error = t.variant({
  "no-such-store": null,
  "access-denied": null,
  other: t.string,
});
const ty = t.result(t.option(t.list(t.u8)), error);

const bytes = encode(ty, { tag: "ok", val: new TextEncoder().encode("hi") });
const value = decode(ty, bytes); // { tag: "ok", val: Uint8Array }
```

To invoke a function, build the request bytes with `encodeInvocation`, write
them on a transport stream, and decode the reply with `decodeResults`:

```js
import { t, encodeInvocation, decodeResults, concatBytes } from "@bytecodealliance/wrpc";

const store = "wasi:keyvalue/store@0.2.0-draft2";

// open: func(identifier: string) -> result<bucket, error>
const payload = encodeInvocation(store, "open", [t.string], [""]);

// ... write `payload` on a bidirectional byte stream and read the reply ...
const reply = concatBytes(chunks);

const [opened] = decodeResults(reply, [t.result(t.own("bucket"), error)]);
if (opened.tag === "err") throw opened.val;
const bucket = opened.val; // a Uint8Array handle
```

The framing matches the default `wrpc-transport` framing (TCP, Unix,
WebTransport, WebSocket): a single connection per invocation, with the protocol
byte, the instance and function names, and a root parameter frame on the way
out, and a root result frame on the way back.

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
| `own<R>` / `borrow<R>`    | `Uint8Array` (the opaque handle bytes)       |

## Working without static types

If a server renders its interface as inlined WIT (as the `wrpc-wasmtime` CLI
does), `parseWit` / `parseType` turn that text into the same type tree the
`t` builders produce, so you can drive the codec without generated bindings:

```js
import { parseType, encode, decode } from "@bytecodealliance/wrpc";

const ty = parseType("result<option<list<u8>>, variant { not-found, other(string) }>");
const value = decode(ty, encode(ty, { tag: "ok", val: new Uint8Array([1, 2, 3]) }));
```

## API

- `t` / `types` — type-tree builders (`bool`, `list`, `record`, `result`, …).
- `encode(ty, value)` / `decode(ty, bytes)` — single-value codec.
- `encodeValue(writer, ty, value)` / `decodeValue(reader, ty)` — streaming codec.
- `Writer` / `Reader` — LEB128 and `core:name` byte primitives.
- `parseType(text)` / `parseWit(text)` — WIT text → type tree.
- `encodeInvocation(instance, func, paramTypes, args)` — the request bytes.
- `decodeResults(bytes, resultTypes)` — the decoded results.
- `concatBytes(chunks)` — join the byte chunks a transport yields.

## Development

```sh
npm test       # codec / framing / WIT round-trip tests (node --test)
npm run check  # type-check the sources and declarations (tsc)
```

## License

Apache-2.0 WITH LLVM-exception
