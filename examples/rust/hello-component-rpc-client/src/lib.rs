mod bindings {
    use crate::Command;

    wit_bindgen::generate!({ generate_all });
    export!(Command);
}

use bindings::wrpc::rpc;

struct Command;

impl bindings::exports::wasi::cli::run::Guest for Command {
    async fn run() -> Result<(), ()> {
        implicit().await;
        explicit().await;
        Ok(())
    }
}

async fn explicit() {
    let cx = rpc::context::Context::default();
    let (_, rx, invocation) =
        rpc::invoker::invoke(cx, "wrpc-examples:hello/handler", "hello", &[], &[]);
    let results = async move {
        let (data, res) = rx.read_via_stream()?;
        drop(rx);
        let greeting = data.collect().await;
        res.await?;
        Ok::<_, rpc::error::Error>(greeting)
    };
    let (greeting, invocation) = futures::join!(results, invocation.into_future());
    let greeting = match greeting {
        Ok(greeting) => greeting,
        Err(err) => {
            eprintln!("failed to read `hello` results: {}", err.to_debug_string());
            return;
        }
    };
    if let Err(err) = invocation {
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
    if greeting.len() != usize::from(*n) {
        eprintln!("failed to read `hello` results: invalid string length");
        return;
    }
    println!(
        "explicit call result: {}",
        String::from_utf8_lossy(greeting)
    );
}

async fn implicit() {
    match bindings::wrpc_examples::hello::handler::hello().await {
        Ok(greeting) => println!("implicit call result: {greeting}"),
        Err(err) => eprintln!(
            "failed to invoke `hello` implicitly: {}",
            err.to_debug_string()
        ),
    }
}
