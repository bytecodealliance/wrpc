use anyhow::Context as _;
use clap::Parser;
use std::sync::Arc;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate
        }
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Prefixes to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "rust")]
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { prefixes } = Args::parse();

    let session = wrpc_cli::zenoh::connect()
        .await
        .context("failed to connect to zenoh")?;

    let arc_session = Arc::new(session);

    for prefix in prefixes {
        let wrpc = wrpc_transport_zenoh::Client::new(arc_session.clone(), prefix.clone())
            .await
            .context("failed to construct transport client")?;
        let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
            .await
            .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
        eprintln!("{prefix}: {hello}");
    }

    Ok(())
}
