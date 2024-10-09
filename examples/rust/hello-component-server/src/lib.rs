mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
    export!(Handler);
}

struct Handler;

impl bindings::exports::wrpc_examples::hello::handler::Guest for Handler {
    fn hello() -> String {
        "hello from Rust".to_string()
    }
}
