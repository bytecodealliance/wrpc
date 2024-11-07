# `wasi-keyvalue` in the Web

This example shows a Web app using `wasi:keyvalue` implementation provided by the server via WebTransport using wRPC.

The server acts as both a direct implementor of `wasi:keyvalue` and a "proxy", which can delegate `wasi:keyvalue` calls issued by the Web client to other `wasi:keyvalue` implementations via various transports supported by wRPC.

## Runing

With `cargo`:

```
cargo run -p wasi-keyvalue-web
```

## `wasi-keyvalue` plugins

To invoke other wRPC `wasi:keyvalue` plugins in "proxy" mode, for example TCP, try:

```
cargo run -p wasi-keyvalue-tcp-server
```

And select `wRPC/TCP` as the protocol in the UI
