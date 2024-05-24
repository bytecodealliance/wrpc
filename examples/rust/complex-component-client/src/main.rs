use crate::bindings::wrpc_examples::complex::resources;

#[allow(clippy::missing_safety_doc)]
mod bindings {
    wit_bindgen::generate!();
}

fn main() {
    let foo = resources::Foo::new();
    println!("foo.bar: {}", foo.bar());
    println!("resources.bar: {}", resources::bar(&foo));
}
