crate::generate!({
    inline: r#"
        package example:imported-resources;

        world import-some-resources {
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

    // provided to satisfy imports, since `wit_bindgen_wrpc` crate is not imported here.
    // not required for external use.
    anyhow_path: anyhow,
    bytes_path: bytes,
    futures_path: futures,
    wrpc_transport_path: wrpc_transport,
});
