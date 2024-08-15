crate::generate!({
    inline: r#"
        package example:world-imports;

        world with-imports {
            /// Fetch a greeting to present.
            import greet: func() -> string;

            /// Log a message to the host.
            import log: func(msg: string);

            import my-custom-host: interface {
                tick: func();
            }
        }
    "#,

    // provided to satisfy imports, since `wit_bindgen_wrpc` crate is not imported here.
    // not required for external use.
    anyhow_path: "anyhow",
    wrpc_transport_path: "wrpc_transport",
});
