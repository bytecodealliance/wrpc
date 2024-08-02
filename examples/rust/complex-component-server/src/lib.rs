#[allow(clippy::missing_safety_doc)]
mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
       with: {
           "wrpc-examples:complex/resources": generate
       }
    });

    export!(Handler);
}

use bindings::exports::wrpc_examples::complex::resources::FooBorrow;

pub struct Handler;

pub struct Foo;

impl bindings::exports::wrpc_examples::complex::resources::GuestFoo for Foo {
    fn new() -> Self {
        Self
    }

    fn foo(_: bindings::exports::wrpc_examples::complex::resources::Foo) -> String {
        "foo".to_string()
    }

    fn bar(&self) -> String {
        "bar".to_string()
    }
}

impl bindings::exports::wrpc_examples::complex::resources::Guest for Handler {
    type Foo = Foo;

    fn bar(_: FooBorrow<'_>) -> String {
        "bar".to_string()
    }
}
