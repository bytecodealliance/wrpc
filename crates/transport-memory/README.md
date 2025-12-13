# wRPC In-Memory Transport

This crate provides an in-memory transport for wRPC that allows components running in the same process to communicate without requiring network connections.

Internally uses `tokio::io::duplex` to create bidirectional in-memory streams

## Use Case

This is particularly useful when
1. you have a "host" component that orchestrates multiple "child" components. Instead of each child requiring its own network connection, you can use in-memory streams to route calls internally.
2. Debugging and testing 

## Example

```rust
use wrpc_transport_memory::new_memory_transport;

// Set up in-memory transport
let (client, server) = new_memory_transport();

// register your handlers functions you care about
server.serve_function_shared(
    store,
    instance,
    guest_resources,
    host_resources,
    func_ty,
    "instance-name",
    "function-name",
).await?;

/* Spawn tasks to process invocations for this function */

// Spawn task to accept connections
tokio::spawn(async move {
    server.accept_loop().await?;
    Ok::<(), anyhow::Error>(())
});

let (mut outgoing, mut incoming) = client
    .invoke((), instance_name, func_name, bytes::Bytes::new(), paths)
    .await;
outgoing.flush().await;

// three options:
incoming.read_i64_leb128().await; // Rust value
incoming.read_value(&mut store, &ty, guest_resources, path).await; // wasmtime Val
incoming.read_raw_bytes.await; // wRPC value encoding
```
