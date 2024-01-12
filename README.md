This is a very messy draft-of-a-draft-of-a-draft distributed WIT-based RPC PoC.

There is a bunch of irrelevant stuff here for now, work is underway - expect the unexpected and prepare to be wholeheartedly underwhelmed.

Only things worth looking at are at:
- `examples`
- `src/bin/wrpc-server.rs`
- `src/nats.rs`

To run the uninspiring demo:

```
$ cargo build -p example-foo --target wasm32-wasi
$ cargo build -p example-foobar --target wasm32-wasi
$ cargo build -p example-run --target wasm32-wasi
```

Then, in separate terminals:

```
$ cargo run --bin wrpc-server ./target/wasm32-wasi/debug/example_foobar.wasm
```

```
$ cargo run --bin wrpc-server ./target/wasm32-wasi/debug/example_foo.wasm
```

```
$ cargo run --bin wrpc-server ./target/wasm32-wasi/debug/example_run.wasm
```

```
$ nats sub >
```

```
$ nats request 'wrpc.0.0.1.wrpc:examples/run@0.1.0/run' ''
```

You should see something like this:
```
21:16:01 Subscribing on > 
[#1] Received on "wrpc.0.0.1.wrpc:examples/run@0.1.0/run" with reply "_INBOX.092ylA9CG6jSjWMfMFyoHk.4EO5m8ta"
nil body


[#2] Received on "_INBOX.092ylA9CG6jSjWMfMFyoHk.4EO5m8ta" with reply "_INBOX.pFHr83jDecbQ9AEqS3OzqW"
nil body


[#3] Received on "wrpc.0.0.1.wrpc:examples/foo@0.1.0/foo" with reply "_INBOX.pFHr83jDecbQ9AEqS3OzrK"
nil body


[#4] Received on "_INBOX.pFHr83jDecbQ9AEqS3OzrK" with reply "_INBOX.BPMuSoh5nK7CSxk4LMtua5"
nil body


[#5] Received on "_INBOX.pFHr83jDecbQ9AEqS3OzrK"
Content-Range: bytes 0-30/30

_INBOX.BPMuSoh5nK7CSxk4LMtub2


[#6] Received on "wrpc.0.0.1.wrpc:examples/foobar@0.1.0/foobar" with reply "_INBOX.pFHr83jDecbQ9AEqS3Ozs8"
_INBOX.BPMuSoh5nK7CSxk4LMtub2


[#7] Received on "_INBOX.pFHr83jDecbQ9AEqS3Ozs8" with reply "_INBOX.SPGmfuuZJwtIkvuQOt1IJP"
nil body


[#8] Received on "_INBOX.BPMuSoh5nK7CSxk4LMtub2.get-unwrap" with reply "_INBOX.SPGmfuuZJwtIkvuQOt1IKb"
nil body


[#9] Received on "_INBOX.SPGmfuuZJwtIkvuQOt1IKb" with reply "_INBOX.BPMuSoh5nK7CSxk4LMtubz"
nil body


[#10] Received on "_INBOX.SPGmfuuZJwtIkvuQOt1IKb"
Content-Range: bytes 0-4/4

foo


[#11] Received on "_INBOX.pFHr83jDecbQ9AEqS3Ozs8"
Content-Range: bytes 0-7/7

foobar


[#12] Received on "_INBOX.092ylA9CG6jSjWMfMFyoHk.4EO5m8ta"
Content-Range: bytes 0-7/7

foobar
```

The last message here (`foobar`) is the response to the initial `wrpc.0.0.1.wrpc:examples/run@0.1.0/run` call.
