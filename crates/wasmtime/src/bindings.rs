mod generated {
    wasmtime::component::bindgen!({
        world: "wrpc:rpc/imports",
        with: {
            "wrpc:rpc/context.context": with::Context,
            "wrpc:rpc/error.error": crate::rpc::Error,
            "wrpc:rpc/transport.incoming-channel": crate::rpc::IncomingChannel,
            "wrpc:rpc/transport.outgoing-channel": crate::rpc::OutgoingChannel,
        },
        imports: {
            "wrpc:rpc/transport.[method]incoming-channel.read-via-stream": store | trappable,
            "wrpc:rpc/transport.[method]outgoing-channel.write-via-stream": store | trappable,
            "wrpc:rpc/invoker.invoke": store | trappable,
            default: trappable,
        },
        require_store_data_send: true,
    });

    pub mod with {
        use core::any::Any;

        #[repr(transparent)]
        pub struct Context(pub Box<dyn Any + Send>);
    }
}

pub use generated::wrpc::rpc;
