pub mod a {
    ::wit_bindgen_wrpc::generate!({
        world: "a",
        path: "./test.wit",
    });
}

pub mod b {
    ::wit_bindgen_wrpc::generate!({
        world: "b",
        path: "./test.wit",
    });
}
