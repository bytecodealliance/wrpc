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

See [specification](./SPEC.md) for more info.

## Installation

- Using [`cargo`](https://doc.rust-lang.org/cargo/index.html):

    ```sh
    cargo install wrpc
    ```

- Using [`nix`](https://zero-to-nix.com/start/install):

    ```sh
    nix profile install github:bytecodealliance/wrpc
    ```

    or, without installing:
    ```
    nix shell github:bytecodealliance/wrpc
    ```

- You can also download individual binaries from the [release page](https://github.com/bytecodealliance/wrpc/releases)

## Quickstart

wRPC usage examples for different programming languages can be found at [examples](./examples).

There are 2 different kinds of examples:
- Native wRPC applications, tied to a particular wRPC transport (like Unix Domain Sockets, TCP, QUIC, WebTransport or WebSockets)
- Generic Wasm components, that need to run in a Wasm runtime. Those can be executed, for example, using `wrpc-wasmtime`, to polyfill imports at runtime and serve exports using wRPC.

### Requirements

- For Rust components and wRPC applications: `rust` >= 1.93

Nix users can run `nix develop` anywhere in the repository to get all dependencies correctly set up

### `hello` example

In this example we will serve and invoke a simple [`hello`](./examples/wit/hello/hello.wit) application.

#### Rust components

We will use the following two Rust components:
- [examples/rust/hello-component-client](examples/rust/hello-component-client)
- [examples/rust/hello-component-server](examples/rust/hello-component-server)

We will have to build these components first:

- Build Wasm `hello` client:

    ```sh
    cargo build --release -p hello-component-client --target wasm32-wasip2
    ```

    > Output is in target/wasm32-wasip2/release/hello-component-client.wasm

- Build Wasm `hello` server:

    ```sh
    cargo build --release -p hello-component-server --target wasm32-wasip2
    ```
    
    > Output is in target/wasm32-wasip2/release/hello_component_server.wasm

    > NB: Rust uses `_` separators in the filename, because a component is built as a reactor-style library

#### Using TCP transport

We will use the following two Rust wRPC applications using TCP transport:
- [examples/rust/hello-tcp-client](examples/rust/hello-tcp-client)
- [examples/rust/hello-tcp-server](examples/rust/hello-tcp-server)

> `[::1]:7761` is used as the default address

1. Serve Wasm `hello` server via TCP

    ```sh
    wrpc-wasmtime tcp serve ./target/wasm32-wasip2/release/hello_component_server.wasm
    ```

    - Sample output:
    > INFO wrpc_wasmtime_cli: serving instance function name="hello"

3. Call Wasm `hello` server using a Wasm `hello` client via TCP:

    ```sh
    wrpc-wasmtime tcp run ./target/wasm32-wasip2/release/hello-component-client.wasm
    ```

    - Sample output in the client:
    >hello from Rust

    - Sample output in the server:
    > INFO wrpc_wasmtime_cli: serving instance function invocation
    >
    > INFO wrpc_wasmtime_cli: successfully served instance function invocation

4. Call the Wasm `hello` server using a native wRPC `hello` client via TCP:

    ```sh
    cargo run -p hello-tcp-client
    ```

5. Serve native wRPC `hello` server via TCP:

    ```sh
    cargo run -p hello-tcp-server [::1]:7762
    ```

6. Call native wRPC `hello` server using native wRPC `hello` client via TCP:

    ```sh
    cargo run -p hello-tcp-client [::1]:7762
    ```

7. Call native wRPC `hello` server using Wasm `hello` client via TCP:

    ```sh
    wrpc-wasmtime tcp run --import [::1]:7762 ./target/wasm32-wasip2/release/hello-component-client.wasm
    ```

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

👋 **Welcome, new contributors!**

Whether you're a seasoned developer or just getting started, your contributions are valuable to us. Don't hesitate to jump in, explore the project, and make an impact. To start contributing, please check out our [Contribution Guidelines](CONTRIBUTING.md). 
