# wRPC v0.0.1-draft.1 specification

wRPC is a transport-agnostic protocol designed for asynchronous transmit of WIT function call invocations over network and other means of communication.

wRPC follows client-server model, where peers (servers) may *serve* function and method calls invoked by the other peers (clients).

wRPC relies on [component model value definition encoding] for data encoding on the wire.

## Definitions

### Transport

wRPC makes use of *transports*, which are responsible for establishing a connection between two parties and transferring the wRPC wire protocol data between them.

Examples of supported wRPC transports are: TCP, Unix Domain Sockets, QUIC, WebTransport and WebSockets.

### Indexing

As WIT interfaces and associated values are asynchronous in nature, callers and callees require ability to asynchronously, bidirectionally transfer (portions of) function parameter and result value data.

For example, caller of `wasi:http/outgoing-handler.handle` function MUST be able to simultaneously send data over the passed `output-stream` and well as receive data from the returned `input-stream`. For this purpose wRPC *multiplexes* these data streams over a single bidirectional byte stream provided by the transport, allowing for bidirectional concurrent transfer of multiple data streams (see [Framing](#framing)).

wRPC uses a concept of "indexing" for differentiating and identifying the data streams used as part of processing of a single WIT function invocation.

An "index" is a sequence of unsigned 32-bit integers and represents a reflective structural path to the value, e.g. a record field or a list element.

Consider the following WIT:

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

A path to field `a` in `foo` parameter `v` is defined as a sequence: `[0, 0]`.

The invoker of `foo` MAY choose to send the whole contents of parameter `v`, i.e. `rec` encoded using [component model value definition encoding] on the "root" (synchronous) parameter data channel, in which case `stream<u8>` is sent as `ready` `list<u8>` (see [Streams](#streams)), otherwise, the invoker MAY mark `rec.a` as pending in the encoding of `v` and instead, send it asynchnously over data channel identified by `[0, 0]` (first field in the first parameter).

Similarly, the handler of `foo` MAY either send the complete resulting `stream<u8>` contents as encoded `ready` `list<u8>` over the "root" (synchronous) result data channel or asynchronously over data channel identified by `0` (first return value).

The indexing rules are as follows:

- Record fields are indexed in order of their WIT declaration
- Tuple members are indexed in order of their WIT declaration
- Variant members elements are indexed in order of their WIT declaration
- List elements are indexed in the order they appear in the list
- Stream elements are indexed in the order they appear in the stream

### Framing

Transports provide a single bidirectional byte stream per wRPC invocation. wRPC defines a mandatory framing format, described below, that multiplexes the asynchronous data streams of an invocation (each identified by its [index](#indexing)) over that single byte stream.

All wRPC transports use this framing. A transport implementation only needs to provide a single bidirectional byte stream; the framing layer supplies the multiplexing, so transports do not implement their own indexing scheme.

## Framed stream specification

wRPC framed stream begins with a version byte `0x00` and is followed by a header encoded using [component model value definition encoding]:

```wit
record header {
    instance: string,
    name: string,
}
```

The header MAY be followed by one or more frames encoded using [component model value definition encoding]:

```wit
record frame {
    path: list<u32>,
    data: list<u8>,
}
```

Frames do not carry any notion of closing the data sent on a `path`: the end of that data is determined by its encoding, for example, a pending stream ends with an empty `list<T>` chunk (see [Streams](#streams)), which is encoded as non-empty `data`.

The `data` of a frame MUST NOT be empty: a sender MUST NOT send a frame with empty `data`, and a receiver MUST treat such a frame as an error. Frames therefore cannot communicate the end of transmit, i.e. that no more frames follow. Each transport MUST communicate it using its own mechanism, for example, TCP shutdown or finishing a QUIC stream.

## Transport specifications

### TCP

TCP relies on [Framed stream specification](#framed-stream-specification) to map a single TCP stream to a single wRPC invocation.

The server MUST listen on a TCP socket and client MUST establish a new connection to that socket per each invocation.

The write side of the stream MUST be shutdown as soon as data transfer is done, for example, once the client is done sending encoded parameter buffer and all asynchronous parameters, it MUST shutdown the write side of the stream to signal EOF to the server.

## Component model value definition encoding extensions

### Futures

`future<T>` values are encoded as `variant future<T> { pending, ready(T) }`.

In case a future is pending, it's value is transmitted using the parent's index.

For example:
```wit
    foo: func(v: future<bool>);
```

If `v` is pending, encoded `bool` value is sent on index `0` (corresponding to first parameter)

### Streams

`stream<T>` values are encoded as `variant stream<T> { pending, ready(list<T>) }`.

A `ready` stream contains all of the stream's elements, i.e. the stream is finished. An empty `ready` stream is a finished stream containing no elements.

In case a stream is pending, it is transmitted as a sequence of `list<T>` chunks using the parent's index.

Each pending stream MUST finish with an empty `list<T>`. Because an empty chunk marks the end of the stream, a sender MUST NOT send an empty chunk before the stream is finished.

### Resources

Resources are encoded as opaque byte blobs, `list<u8>` and their meaning is entirely application specific.

[component model value definition encoding]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#-value-definitions
