mod bindings {
    wit_bindgen::generate!({ generate_all });
}

use bindings::wrpc::rpc;

fn explicit() {
    let cx = rpc::context::Context::default();
    let hello = rpc::invoker::invoke(cx, "wrpc-examples:hello/handler", "hello", &[], &[]);
    hello.subscribe().block();
    match rpc::transport::Invocation::finish(hello) {
        Ok((_, rx)) => {
            let rx = rx.data().expect("failed to get input stream");
            match rx.blocking_read(1024) {
                Ok(greeting) => {
                    let Some((n, greeting)) = greeting.split_first() else {
                        eprintln!(
                            "failed to read `hello` results: invalid number of bytes returned"
                        );
                        return;
                    };
                    if *n > 127 {
                        eprintln!(
                            "failed to read `hello` results: string lengths over 127 bytes not currently supported"
                        );
                        return;
                    }
                    println!(
                        "explicit call result: {}",
                        String::from_utf8_lossy(greeting)
                    )
                }
                Err(bindings::wasi::io::streams::StreamError::Closed) => {
                    eprintln!("failed to read `hello` results: stream closed")
                }
                Err(bindings::wasi::io::streams::StreamError::LastOperationFailed(err)) => {
                    match rpc::error::Error::from_io_error(err) {
                        Ok(err) => eprintln!(
                            "failed to read `hello` results due to wRPC error: {}",
                            err.to_debug_string()
                        ),
                        Err(err) => eprintln!(
                            "failed to read `hello` results due to I/O error: {}",
                            err.to_debug_string()
                        ),
                    }
                }
            };
        }
        Err(err) => {
            eprintln!(
                "failed to invoke `hello` explicitly: {}",
                err.to_debug_string()
            )
        }
    }
}

fn implicit() {
    match bindings::wrpc_examples::hello::handler::hello() {
        Ok(greeting) => println!("implicit call result: {greeting}"),
        Err(err) => eprintln!(
            "failed to invoke `hello` implicitly: {}",
            err.to_debug_string()
        ),
    }
}

fn main() {
    implicit();
    explicit();
}
