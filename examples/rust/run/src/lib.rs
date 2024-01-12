use crate::wrpc::examples;

wit_bindgen::generate!({
    exports: {
        "wrpc:examples/run": Example,
    },
});

pub struct Example;

impl exports::wrpc::examples::run::Guest for Example {
    fn run() -> String {
        examples::foobar::foobar(examples::foo::foo())
    }
}
