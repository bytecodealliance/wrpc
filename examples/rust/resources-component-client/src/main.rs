use crate::bindings::wrpc_examples::resources::resources;

mod bindings {
    wit_bindgen::generate!({
       with: {
           "wrpc-examples:resources/resources": generate
       }
    });
}

fn main() {
    let resource = resources::Foo::new();
    println!("resources.bar: {}", resources::bar(&resource));
    println!("foo.bar: {}", resource.bar());
    println!("foo.foo: {}", resources::Foo::foo(resource));
}
