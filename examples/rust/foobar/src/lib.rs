use wrpc::examples::types::FutureString;

wit_bindgen::generate!({
    exports: {
        "wrpc:examples/foobar": Example,
    },
});

pub struct Example;

impl exports::wrpc::examples::foobar::Guest for Example {
    fn foobar(s: FutureString) -> String {
        // TODO(2): Use pollable
        //s.subscribe().block();

        // TODO(1): Use an option
        //loop {
        //    match s.get() {
        //        None => {
        //            eprintln!("sleep for 1ms");
        //            sleep(Duration::from_millis(1));
        //        }
        //        Some(s) => return format!("{s}bar"),
        //    }
        //}

        format!("{}bar", s.get_unwrap())
    }
}
