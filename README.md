# wRPC

`wRPC` is a [component]-native transport-agnostic RPC protocol and framework based on [WebAssembly Interface Types (WIT)].
If facilitates execution of arbitrary functionality defined in [WIT] over network or other means of communication.

Main use cases for wRPC are:
- out-of-tree WebAssembly runtime plugins
- distributed WebAssembly component communication

Even though wRPC is designed for Wasm components first and foremost, it is fully usable outside of WebAssembly context and can serve as a general-purpose RPC framework.

wRPC uses component model value encoding (fully defined and pending spec merge at https://github.com/WebAssembly/component-model/pull/336) on the wire.

wRPC supports both dynamic (based on e.g. runtime WebAssembly component type introspection) and static use cases.

For static use cases, wRPC provides [WIT] binding generators for:
- Rust
- Go

wRPC fully supports the unreleased native [WIT] `stream` and `future` data types along with all currently released WIT functionality.

## Design

### Transport

wRPC transport is the core abstraction on top of which all the other functionality is built.

A transport represents a multiplexed bidirectional communication channel, over which wRPC invocations are transmitted.

wRPC operates under assumption that transport communication channels can be "indexed" by a sequence of unsigned 32-bit integers, which represent a reflective structural path.

### Invocation

As part of every wRPC invocation at least 2 independent, directional byte streams will be established by the chosen transport:

- parameters (client -> server)
- results (server -> client)

wRPC transport implementations MAY (and are encouraged to) provide two more directional communication channels:

- client error (client -> server)
- server error (server -> client)

Error channels are the only channels that are *typed*, in particular, values sent on these channels are *strings*.

If `async` values are being transmitted as parameters or results of an invocation, wRPC MAY send those values on an indexed path asynchronously.

Consider the invocation of [WIT] function `foo` from instance `wrpc-example:doc/example@0.1.0`:

```wit
package wrpc-example:doc@0.1.0;

interface example {
    record rec {
        a: stream<u8>,
        b: u32,
    }

    foo: func(v: rec) -> stream<u8>;
}
```

1. Since `foo` parameter `0` is a `record`, which contains an `async` type (`stream`) as the first field, wRPC will communicate to the transport that apart from the "root" parameter channel, it may need to receive results at index path `0` (first return value).
2. wRPC will encode the parameters as a single-element tuple in a non-blocking fashion. If full contents of `rec.a` are not available at the time of encoding, the stream will be encoded as `option::none`.
5. (concurrently, if in `2.` stream was not fully available) wRPC will transfer the contents of the `stream<u8>` on parameter byte stream at index `0->0` (first field of the record, which is the first parameter) as they become available.
4. wRPC will attempt to decode `stream<u8>` from the "root" result byte stream.
5. (if `4.` decoded an `option::none` for the `stream` value) wRPC will attempt to decode `stream<u8>` from result byte stream at index `0`

Note, that the *handler* of `foo` (server) MAY:
- receive `rec.b` value before `rec.a` is sent or even available
- send a result back to the *invoker* of `foo` (client) *before* it has received `rec.a`

## Repository structure

This repository contains (for all supported languages):
- core libraries and abstractions
- binding generators
- WebAssembly runtime integrations
- wRPC transport implementations

`wit-bindgen-wrpc` aims to closely match UX of [`wit-bindgen`] and therefore includes a subtree merge of the project, which is occasionally merged into this tree.
- wRPC binding generators among other tests, are tested using the [`wit-bindgen`] test suite
- [`wit-bindgen`] documentation is reused where applicable

[`wit-bindgen`]: https://github.com/bytecodealliance/wit-bindgen
[component]: https://component-model.bytecodealliance.org/
[WebAssembly Interface Types (WIT)]: https://component-model.bytecodealliance.org/design/wit.html
[WIT]: https://component-model.bytecodealliance.org/design/wit.html
