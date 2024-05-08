mod bindings {
    wit_bindgen::generate!();
}

fn main() {
    let greeting = bindings::wrpc_examples::hello::handler::hello();
    println!("{greeting}");
}
