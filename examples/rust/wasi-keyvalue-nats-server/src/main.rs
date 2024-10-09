use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
use futures::stream::select_all;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::{select, signal};
use tracing::{info, warn};
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefix to serve `wasi:keyvalue` on
    #[arg(default_value = "rust")]
    prefix: String,
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

    let invocations = wrpc_wasi_keyvalue::serve(
        &wrpc_transport_nats::Client::new(nats, prefix, None),
        wrpc_wasi_keyvalue_mem::Handler::default(),
    )
    .await
    .context("failed to serve `wasi:keyvalue`")?;
    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per export
    let mut invocations = select_all(invocations.into_iter().map(
        |(instance, name, invocations)| {
            invocations
                .try_buffer_unordered(16) // handle up to 16 invocations concurrently
                .map(move |res| (instance, name, res))
        },
    ));
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(()) => {
                        info!(instance, name, "invocation successfully handled");
                    }
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            }
            res = &mut shutdown => {
                return res.context("failed to listen for ^C")
            }
        }
    }
}
