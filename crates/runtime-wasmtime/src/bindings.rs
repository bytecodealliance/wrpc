mod generated {
    wasmtime::component::bindgen!({
        world: "wrpc:rpc/imports",
        with: {
            "wasi": wasmtime_wasi::p2::bindings,
            "wrpc:rpc/context.context": with::Context,
            "wrpc:rpc/error.error": crate::rpc::Error,
            "wrpc:rpc/transport.incoming-channel": crate::rpc::IncomingChannel,
            "wrpc:rpc/transport.invocation": crate::rpc::Invocation,
            "wrpc:rpc/transport.outgoing-channel": crate::rpc::OutgoingChannel,
        },
        imports: {
            default: trappable,
            "wrpc:rpc/transport@0.1.0.[static]invocation.finish": async | trappable,
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
