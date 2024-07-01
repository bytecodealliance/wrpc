#[allow(clippy::missing_safety_doc)]
mod bindings {
    wit_bindgen::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

fn main() {
    let greeting = bindings::wrpc_examples::hello::handler::hello();
    println!("{greeting}");
}
