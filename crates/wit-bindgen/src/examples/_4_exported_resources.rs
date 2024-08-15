crate::generate!({
    inline: r#"
        package example:exported-resources;

        world import-some-resources {
            export logging;
        }

        interface logging {
            enum level {
                debug,
                info,
                warn,
                error,
            }
            resource logger {
                constructor(max-level: level);

                get-max-level: func() -> level;
                set-max-level: func(level: level);

                log: func(level: level, msg: string);
            }
        }
    "#,

    // provided to satisfy imports, since `wit_bindgen_wrpc` crate is not imported here,
    // not required for external use.
    anyhow_path: "anyhow",
    bytes_path: "bytes",
    futures_path: "futures",
    tokio_path: "tokio",
    tokio_util_path: "tokio_util",
    tracing_path: "tracing",
    wasm_tokio_path: "wasm_tokio",
    wrpc_transport_path: "wrpc_transport",
});
