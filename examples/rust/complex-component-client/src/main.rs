use crate::bindings::wrpc_examples::complex::resources;

#[allow(clippy::missing_safety_doc)]
mod bindings {
    wit_bindgen::generate!({
       with: {
           "wrpc-examples:complex/resources": generate
       }
    });
}

fn main() {
    let resource = resources::Foo::new();
    println!("foo.bar: {}", resource.bar());
    println!("resources.bar: {}", resources::bar(&resource));
}
