mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        path: "../wit",
        world: "handler",
        with: {
            "wrpc-bench:bench/ping": generate,
            "wrpc-bench:bench/greet": generate,
        },
    });
    export!(Handler);
}

pub struct Handler;

impl bindings::exports::wrpc_bench::bench::ping::Guest for Handler {
    fn ping() {}
}

impl bindings::exports::wrpc_bench::bench::greet::Guest for Handler {
    fn greet(name: String) -> String {
        format!("Hello, {name}")
    }
}
