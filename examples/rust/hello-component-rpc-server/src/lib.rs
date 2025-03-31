mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({ generate_all });
    export!(Handler);
}

type Result<T, E = bindings::wrpc::rpc::error::Error> = core::result::Result<T, E>;

struct Handler;

impl bindings::exports::wrpc_examples::hello::handler::Guest for Handler {
    fn hello() -> Result<String> {
        Ok("hello from Rust".to_string())
    }
}
