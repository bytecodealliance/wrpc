use wasi::io::poll::Pollable;
use wit_bindgen::Resource;

wit_bindgen::generate!({
    exports: {
        "wrpc:examples/foo": Example,
        "wrpc:examples/types/future-string": Foo,
    },
});

pub struct Foo;

impl exports::wrpc::examples::types::GuestFutureString for Foo {
    fn subscribe(&self) -> Pollable {
        crate::wasi::clocks::monotonic_clock::subscribe_duration(1)
    }

    fn get(&self) -> Option<String> {
        Some(String::from("foo"))
    }

    // TODO: Remove
    fn get_unwrap(&self) -> String {
        String::from("foo")
    }
}

pub struct Example;

impl exports::wrpc::examples::foo::Guest for Example {
    fn foo() -> Resource<Foo> {
        Resource::new(Foo)
    }
}
