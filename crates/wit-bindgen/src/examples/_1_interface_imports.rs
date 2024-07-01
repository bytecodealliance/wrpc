crate::generate!({
    inline: r#"
        package example:interface-imports;

        interface logging {
            enum level {
                debug,
                info,
                warn,
                error,
            }

            log: func(level: level, msg: string);
        }

        world with-imports {
            // Local interfaces can be imported.
            import logging;

            // Dependencies can also be referenced, and they're loaded from the
            // `path` directive specified below.
            import wasi:cli/environment@0.2.0;
        }
    "#,

    // provided to satisfy imports, since `wit_bindgen_wrpc` crate is not imported here.
    // not required for external use.
    anyhow_path: anyhow,
    async_trait_path: async_trait,
    bytes_path: bytes,
    futures_path: futures,
    wrpc_transport_path: wrpc_transport,

    path: "wasi-cli@0.2.0.wasm",

    // specify that this interface dependency should be generated as well.
    with: {
        "wasi:cli/environment@0.2.0": generate,
    }
});
