mod bindings {
    use crate::Command;

    wit_bindgen::generate!({ generate_all });
    export!(Command);
}

use bindings::wrpc::rpc;

struct Command;

impl bindings::exports::wasi::cli::run::Guest for Command {
    async fn run() -> Result<(), ()> {
        implicit();
        explicit().await;
        Ok(())
    }
}

async fn explicit() {
    let cx = rpc::context::Context::default();
    let (_, rx, invocation) =
        rpc::invoker::invoke(cx, "wrpc-examples:hello/handler", "hello", &[], &[]);
    let (data, res) = match rx.read_via_stream() {
        Ok(data) => data,
        Err(err) => {
            eprintln!("failed to read `hello` results: {}", err.to_debug_string());
            return;
        }
    };
    let greeting = data.collect().await;
    if let Err(err) = res.await {
        eprintln!("failed to read `hello` results: {}", err.to_debug_string());
        return;
    }
    drop(rx);
    if let Err(err) = invocation.await {
        eprintln!(
            "failed to invoke `hello` explicitly: {}",
            err.to_debug_string()
        );
        return;
    }
    let Some((n, greeting)) = greeting.split_first() else {
        eprintln!("failed to read `hello` results: invalid number of bytes returned");
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
    );
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
