### Requirements *******************

#### Setup zenohd on NixOS

1. Add zenoh to your configuration.nix file in the environment.systemPackages
    ```
    # List packages installed in system profile. To search, run:
    # $ nix search wget
    environment.systemPackages = with pkgs; [
        ...
        zenoh
    ];
    ```

2. Rebuild NixOS
    ```
    sudo nixos-rebuild switch
    ```

3. Test zenohd
    ```
    zenohd
    ```

    - Sample output:
    2025-12-08T12:27:11.419376Z  INFO main ThreadId(01) zenohd: zenohd v1.6.1 built with rustc 1.85.0 (4d91de4e4 2025-02-17)
    2025-12-08T12:27:11.423016Z  INFO main ThreadId(01) zenohd: Initial conf: {"access_control":{"def ........ ,"qos":{"enabled":true}}}}
    2025-12-08T12:27:11.428864Z  INFO main ThreadId(01) zenoh::net::runtime: Using ZID: ba0e4a24bdf641c79bfe9ee37eb9bc4
    2025-12-08T12:27:11.438761Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/[fe80::215:5dff:fe00:b079]:7447
    2025-12-08T12:27:11.439328Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/[fe80::3842:4ff:fe75:baf1]:7447
    2025-12-08T12:27:11.439331Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/[fe80::209d:11ff:fe06:8e06]:7447
    2025-12-08T12:27:11.439333Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/10.255.255.254:7447
    2025-12-08T12:27:11.439334Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/172.21.8.176:7447
    2025-12-08T12:27:11.439336Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: Zenoh can be reached at: tcp/172.18.0.1:7447
    2025-12-08T12:27:11.439428Z  INFO main ThreadId(01) zenoh::net::runtime::orchestrator: zenohd listening scout messages on 224.0.0.224:7446

#### Using [zenoh] transport

We will use the following two Rust wRPC applications using [zenoh] transport:
- [examples/rust/hello-zenoh-client](examples/rust/hello-zenoh-client)
- [examples/rust/hello-zenoh-server](examples/rust/hello-zenoh-server)

1. Run [zenoh]:

    - Build the repo, so that zenoh transport is compiled for wrpc-wasmtime (runtime) using    
    ```sh
    cargo build --release
    ```

    - Create a zenoh config file at /path/to/config/zenoh_conf.json5
    ```sh
    {
        mode: "client",
        listen: {
            endpoints: ["tcp/0.0.0.0:7447"],
        },
    }
    ```
    This is a minimal example config.

    - Set the config environment variable for zenoh:
    > export ZENOH_CONFIG="/path/to/config/zenoh_conf.json5"

    - Running as a daemon with the config above in order to run zenoh as a transport (in Client mode):
    ```sh
    zenohd
    ```

2. Serve Wasm `hello` server via [zenoh]

    ```sh
    ./target/release/wrpc-wasmtime zenoh serve ./target/wasm32-wasip2/debug/hello_component_server.wasm
    ```
    
    - Sample output: 
    >  INFO zenoh::net::runtime: Using ZID: 638a012883d98f769cf4367f8457bc66
    >  INFO zenoh::net::runtime::orchestrator: Scouting...
    >  INFO zenoh::net::runtime::orchestrator: Found HelloProto { version: 9, whatami: Router, zid: f3ae70a723144e6675f4c72d30196797, locators: [tcp/[fe80::215:5dff:fe83:cd20]:7447, tcp/[fe80::8d2:a4ff:fed2:f33d]:7447, tcp/[fe80::c431:38ff:fe58:9310]:7447, tcp/10.255.255.254:7447, tcp/172.21.8.176:7447, tcp/172.18.0.1:7447] }
    >  INFO zenoh::net::runtime::orchestrator: Found HelloProto { version: 9, whatami: Router, zid: f3ae70a723144e6675f4c72d30196797, locators: [tcp/[fe80::215:5dff:fe83:cd20]:7447, tcp/[fe80::8d2:a4ff:fed2:f33d]:7447, tcp/[fe80::c431:38ff:fe58:9310]:7447, tcp/10.255.255.254:7447, tcp/172.21.8.176:7447, tcp/172.18.0.1:7447] }
    >  INFO wrpc_wasmtime_cli: serving instance function name="hello"

3. Call Wasm `hello` server using a Wasm `hello` client via [zenoh]:

    ```sh
    ./target/release/wrpc-wasmtime zenoh run ./target/wasm32-wasip2/debug/hello-component-client.wasm
    ```
    
    - Sample output:
    > INFO zenoh::net::runtime: Using ZID: e4b5dcb134469e2e5773dd88dfb7a8a
    > INFO zenoh::net::runtime::orchestrator: Scouting...
    > INFO zenoh::net::runtime::orchestrator: Found HelloProto { version: 9, whatami: Router, zid: f3ae70a723144e6675f4c72d30196797, locators: [tcp/[fe80::215:5dff:fe83:cd20]:7447, tcp/[fe80::8d2:a4ff:fed2:f33d]:7447, tcp/[fe80::c431:38ff:fe58:9310]:7447, tcp/10.255.255.254:7447, tcp/172.21.8.176:7447, tcp/172.18.0.1:7447] }
    > INFO zenoh::net::runtime::orchestrator: Found HelloProto { version: 9, whatami: Router, zid: f3ae70a723144e6675f4c72d30196797, locators: [tcp/[fe80::215:5dff:fe83:cd20]:7447, tcp/[fe80::8d2:a4ff:fed2:f33d]:7447, tcp/[fe80::c431:38ff:fe58:9310]:7447, tcp/10.255.255.254:7447, tcp/172.21.8.176:7447, tcp/172.18.0.1:7447] }
    hello from Rust
    > INFO zenoh::api::session: close session zid=e4b5dcb134469e2e5773dd88dfb7a8a

4. Call the Wasm `hello` server using a native wRPC `hello` client via [zenoh]:

    ```sh
    cargo run -p hello-zenoh-client
    ```

5. Serve native wRPC `hello` server via [zenoh]:

    ```sh
    cargo run -p hello-zenoh-server
    ```


#### Testing

To test the transport-zenoh package run:

    ```sh
    cargo test zenoh -- --nocapture
    ```

    - Sample output:
    > test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out; finished in 6.17s





























6. Call both the native wRPC `hello` server and Wasm `hello` server using native wRPC `hello` client via [zenoh]:

    ```sh
    cargo run -p hello-zenoh-client rust native
    ```

7. Call native wRPC `hello` server using Wasm `hello` client via [zenoh]:

    ```sh
    wrpc-wasmtime zenoh run --import native ./target/wasm32-wasip2/release/hello-component-client.wasm