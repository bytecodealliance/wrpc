use std::time::Instant;

//WRPC wasm/client to work with hello-zenoh-server(host)

mod bindings {
    wit_bindgen::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

fn main() {
    let start = Instant::now();
    let response_str = bindings::wrpc_examples::hello::handler::hello();
    println!("*** Response: {response_str}");
    let elapsed = start.elapsed();
    println!("*** Elapsed get: {elapsed:?}");
}
