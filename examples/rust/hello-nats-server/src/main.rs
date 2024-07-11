use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use tokio::{select, signal};
use tracing::{info, warn};
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
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

    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per stream
    let mut invocations = bindings::serve(
        &wrpc_transport_nats::Client::new(nats, prefix, None),
        Server,
    )
    .await
    .context("failed to serve `wrpc-examples.hello/handler.hello`")?;
    let invocations = invocations.buffer_unordered(16); // handle at most 16 requests concurrently
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                if let Err(err) = res {
                    warn!(?err, instance, name, "failed to handle invocation");
                } else {
                    info!(instance, name, "invocation handled")
                }
            }
            res = &mut shutdown => {
                return res.context("failed to listen for ^C")
            }
        }
    }
}
