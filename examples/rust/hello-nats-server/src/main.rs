use anyhow::Context as _;
use clap::Parser;
use tokio::signal;
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!();
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefix to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "rust")]
    prefix: String,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::hello::handler::Handler<Option<async_nats::HeaderMap>>
    for Server
{
    async fn hello(&self, _: Option<async_nats::HeaderMap>) -> anyhow::Result<String> {
        Ok("hello from Rust".to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { nats, prefix } = Args::parse();

    let nats = async_nats::connect_with_options(
        String::from(nats),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS.io server")?;

    bindings::serve(
        &wrpc_transport_nats::Client::new(nats, prefix),
        Server,
        async { signal::ctrl_c().await.expect("failed to listen for ^C") },
    )
    .await
    .context("failed to invoke `wrpc-examples.hello/handler.hello`")
}
