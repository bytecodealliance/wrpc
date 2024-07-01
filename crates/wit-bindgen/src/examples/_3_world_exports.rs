crate::generate!({
    inline: r#"
        package example:world-exports;

        world with-exports {
            import log: func(msg: string);

            export run: func();

            /// An example of exporting an interface inline naming it directly.
            export environment: interface {
                get: func(var: string) -> string;
                set: func(var: string, val: string);
            }

            /// An example of exporting an interface defined in this file.
            export units;

            /// An example of exporting an interface defined in a dependency.
            export wasi:random/insecure@0.2.0;
        }

        interface units {
            use wasi:clocks/monotonic-clock@0.2.0.{duration};

            /// Renders the number of bytes as a human readable string.
            bytes-to-string: func(bytes: u64) -> string;

            /// Renders the provided duration as a human readable string.
            duration-to-string: func(dur: duration) -> string;
        }
    "#,

    // provided to satisfy imports, since `wit_bindgen_wrpc` crate is not imported here.
    // not required for external use.
    anyhow_path: anyhow,
    bytes_path: bytes,
    futures_path: futures,
    tokio_path: tokio,
    tracing_path: tracing,
    wrpc_transport_path: wrpc_transport,

    // provided to specify the path to `wasi:*` dependencies referenced above.
    path: "wasi-cli@0.2.0.wasm",

    // specify that these interface dependencies should be generated as well.
    with: {
        "wasi:random/insecure@0.2.0": generate,
        "wasi:clocks/monotonic-clock@0.2.0": generate,
        "wasi:io/poll@0.2.0": generate
    }
});
