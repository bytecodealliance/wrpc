use core::pin::pin;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use tokio::task::JoinSet;
use tokio::{fs, select, signal};
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
    let handler = wrpc_wasi_keyvalue_mem::Handler::default();
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            res = lis.accept() => {
                match res {
                    Ok((stream, ..)) => {
                        debug!("connection accepted");
                        let (rx, tx) = stream.into_split();
                        let handler = handler.clone();
                        tasks.spawn(async move {
                            if let Err(err) = wrpc_wasi_keyvalue::serve((), tx, rx, handler).await {
                                warn!(?err, "failed to serve connection");
                            } else {
                                info!("connection successfully served");
                            }
                        });
                    }
                    Err(err) => {
                        warn!(?err, "failed to accept connection");
                    }
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => return res.context("failed to listen for ^C"),
        }
    }
}
