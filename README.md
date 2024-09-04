<div align="center">
  <h1><code>wRPC</code></h1>

  <p>
    <strong>
    <a href="https://component-model.bytecodealliance.org/">Component-native</a>
    transport-agnostic RPC protocol and framework based on
    <a href="https://component-model.bytecodealliance.org/design/wit.html">WebAssembly Interface Types (WIT)</a>
    </strong>
  </p>

  <strong>A <a href="https://bytecodealliance.org/">Bytecode Alliance</a> hosted project</strong>

  <p>
    <a href="https://github.com/bytecodealliance/wrpc/actions?query=workflow%3Awrpc"><img src="https://github.com/bytecodealliance/wrpc/actions/workflows/wrpc.yml/badge.svg" alt="build status" /></a>
    <a href="https://docs.rs/wrpc"><img src="https://docs.rs/wrpc/badge.svg" alt="Documentation Status" /></a>
  </p>
</div>

## About

wRPC facilitates execution of arbitrary functionality defined in [WIT] over network or other means of communication.

Main use cases for wRPC are:
- out-of-tree WebAssembly runtime plugins
- distributed WebAssembly component communication

Even though wRPC is designed for Wasm components first and foremost, it is fully usable outside of WebAssembly context and can serve as a general-purpose RPC framework.

wRPC uses [component model value definiton encoding] on the wire.

wRPC supports both dynamic (based on e.g. runtime WebAssembly component type introspection) and static use cases.

For static use cases, wRPC provides [WIT] binding generators for:
- Rust
- Go

wRPC fully supports the unreleased native [WIT] `stream` and `future` data types along with all currently released WIT functionality.

## Quickstart

wRPC usage examples for different programming languages can be found at [examples](./examples).

There are 2 different kinds of examples:
- Native wRPC applications, tied to a particular wRPC transport (currently, NATS only)
- Generic Wasm components, that need to run in a Wasm runtime. Those can be executed, for example, using `wrpc-wasmtime-nats`, to polyfill imports at runtime and serve exports using wRPC.

### `hello` example

In this example we will serve and invoke a simple [`hello`](./examples/wit/hello/hello.wit) application using:

- [examples/rust/hello-component-client](examples/rust/hello-component-client)
- [examples/rust/hello-component-server](examples/rust/hello-component-server)
- [examples/rust/hello-nats-client](examples/rust/hello-nats-client)
- [examples/rust/hello-nats-server](examples/rust/hello-nats-server)

#### Requirements

- `nats-server` >= 2.10.20 or `docker` >= 24.0.6 (or any other OCI runtime)
- `rust` >= 1.80.1

#### How-To

In the steps below, `wasm32-wasip1` target will be used, because it is currently available in stable Rust and also conveniently configured in [`rust-toolchain.toml`](./rust-toolchain.toml) in the root of this repository.

[`wasm32-wasip2`](https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip2.html) can be used instead, if desired.

1. Build Wasm `hello` client:

    ```sh
    cargo build --release -p hello-component-client --target wasm32-wasip1
    ```

    > Output is in target/wasm32-wasip1/release/hello-component-client.wasm

2. Build Wasm `hello` server:

    ```sh
    cargo build --release -p hello-component-server --target wasm32-wasip1
    ```
    
    > Output is in target/wasm32-wasip1/release/hello_component_server.wasm

    > NB: Rust uses `_` separators in the filename, because a component is built as a reactor-style library

3. Build the wRPC Wasm runtime:
    
    ```sh
    cargo build --release --bin wrpc-wasmtime-nats
    ```

    > Output is in target/release/wrpc-wasmtime-nats or target/release/wrpc-wasmtime-nats.exe on Windows

4. Run NATS (more thorough documentation available [here](https://docs.nats.io/running-a-nats-service/introduction/running)):

    - using standalone binary:
    ```sh
    nats-server
    ```
    
    - using Docker:
    ```sh
    docker run --rm -it --name nats-server -p 4222:4222 nats:2.10.20-alpine3.20
    ```

5. Serve Wasm `hello` server via NATS:

    ```sh
    ./target/release/wrpc-wasmtime-nats serve rust rust ./target/wasm32-wasip1/release/hello_component_server.wasm
    ```
    
    - Sample output:
    > INFO async_nats: event: connected
    >
    > INFO wrpc_wasmtime_nats_cli: serving instance function name="hello"

6. Call Wasm `hello` server using a Wasm `hello` client via NATS:

    ```sh
    ./target/release/wrpc-wasmtime-nats run rust ./target/wasm32-wasip1/release/hello-component-client.wasm
    ```
    
    - Sample output in the client:
    > INFO async_nats: event: connected
    >
    >hello from Rust
    
    - Sample output in the server:
    > INFO wrpc_wasmtime_nats_cli: serving instance function invocation headers=None
    >
    > INFO wrpc_wasmtime_nats_cli: successfully served instance function invocation

7. Call the Wasm `hello` server using a native wRPC `hello` client:

    ```sh
    cargo run -p hello-nats-client rust
    ```

8. Serve native wRPC `hello` server:

    ```sh
    cargo run -p hello-nats-server native
    ```

9. Call both the native wRPC `hello` server and Wasm `hello` server using native wRPC `hello` client:

    ```sh
    cargo run -p hello-nats-client rust native
    ```

10. Call native wRPC `hello` server using Wasm `hello` client via NATS:

    ```sh
    ./target/release/wrpc-wasmtime-nats run native ./target/wasm32-wasip1/release/hello-component-client.wasm
    ```

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
[component model value definiton encoding]: https://github.com/WebAssembly/component-model/blob/8ba643f3a17eced576d8d7d4b3f6c76b4e4347d7/design/mvp/Binary.md#-value-definitions

## Contributing

[![GitHub repo Good Issues for newbies](https://img.shields.io/github/bytecodealliance/issues/wrpc/good%20first%20issue?style=flat&logo=github&logoColor=green&label=Good%20First%20issues)](https://github.com/bytecodealliance/wrpc/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22) [![GitHub Help Wanted issues](https://img.shields.io/github/bytecodealliance/issues/wrpc/help%20wanted?style=flat&logo=github&logoColor=b545d1&label=%22Help%20Wanted%22%20issues)](https://github.com/bytecodealliance/wrpc/issues?q=is%3Aopen+is%3Aissue+label%3A%22help+wanted%22) [![GitHub Help Wanted PRs](https://img.shields.io/github/issues-pr/bytecodealliance/wrpc/help%20wanted?style=flat&logo=github&logoColor=b545d1&label=%22Help%20Wanted%22%20PRs)](https://github.com/bytecodealliance/wrpc/pulls?q=is%3Aopen+is%3Aissue+label%3A%22help+wanted%22) [![GitHub repo Issues](https://img.shields.io/github/bytecodealliance/issues/wrpc?style=flat&logo=github&logoColor=red&label=Issues)](https://github.com/bytecodealliance/wrpc/issues?q=is%3Aopen)

👋 **Welcome, new contributors!**

Whether you're a seasoned developer or just getting started, your contributions are valuable to us. Don't hesitate to jump in, explore the project, and make an impact. To start contributing, please check out our [Contribution Guidelines](CONTRIBUTING.md). 
