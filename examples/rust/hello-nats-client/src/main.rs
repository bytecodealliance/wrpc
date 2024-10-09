use anyhow::Context as _;
use clap::Parser;
use url::Url;

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
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefixes to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "rust")]
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { nats, prefixes } = Args::parse();

    let nats = async_nats::connect_with_options(
        String::from(nats),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS.io server")?;

    for prefix in prefixes {
        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix.clone(), None);
        let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, None)
            .await
            .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
        eprintln!("{prefix}: {hello}");
    }
    Ok(())
}
