use core::pin::pin;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::task::JoinSet;
use tokio::{fs, select, signal};
use tokio_stream::wrappers::UnixListenerStream;
use tracing::{debug, error, info, warn};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to serve `wasi:keyvalue` on
    #[arg(default_value = "/tmp/wrpc/wasi/keyvalue.sock")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { path } = Args::parse();

    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        fs::create_dir_all(dir)
            .await
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    let lis = tokio::net::UnixListener::bind(&path)
        .with_context(|| format!("failed to bind Unix listener on `{}`", path.display()))?;
    let incoming = UnixListenerStream::new(lis).map_ok(|stream| {
        let (rx, tx) = stream.into_split();
        ((), tx, rx)
    });
    let invocations =
        wrpc_wasi_keyvalue::serve(incoming, wrpc_wasi_keyvalue_mem::Handler::default());
    let mut invocations = pin!(invocations);
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some(res) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        debug!("invocation accepted");
                        tasks.spawn(async move {
                            match fut.await { Err(err) => {
                                warn!(?err, "failed to handle invocation");
                            } _ => {
                                info!("invocation successfully handled");
                            }}
                        });
                    }
                    Err(err) => {
                        warn!(?err, "failed to accept invocation");
                    }
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => {
                // wait for all invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to join task");
                    }
                }
                return res.context("failed to listen for ^C")
            }
        }
    }
}
