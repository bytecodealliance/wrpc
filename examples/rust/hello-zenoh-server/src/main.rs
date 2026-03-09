use core::pin::pin;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::stream::select_all;
use futures::StreamExt as _;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::hello::handler::Handler<()> for Server {
    async fn hello(&self, _: ()) -> anyhow::Result<String> {
        Ok("hello from Rust".to_string())
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Prefix to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "rust")]
    prefix: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { prefix } = Args::parse();

    let session = wrpc_cli::zenoh::connect()
        .await
        .context("failed to connect to zenoh")?;

    let arc_session = Arc::new(session);

    let wrpc = wrpc_transport_zenoh::Client::new(arc_session, prefix)
        .await
        .context("failed to construct transport client")?;
    let invocations = bindings::serve(&wrpc, Server)
        .await
        .context("failed to serve `wrpc-examples.hello/handler.hello`")?;
    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per export
    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        debug!(instance, name, "invocation accepted");
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, "failed to handle invocation");
                            } else {
                                info!(instance, name, "invocation successfully handled");
                            }
                        });
                    }
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task")
                }
            }
            res = &mut shutdown => {
                // wait for all invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to join task")
                    }
                }
                return res.context("failed to listen for ^C")
            }
        }
    }
}
