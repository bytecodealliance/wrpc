use core::pin::pin;

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::Parser;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinSet;
use tokio::{fs, select, signal};
use tracing::{debug, error, info, warn};
use wrpc::{Header, Listener};

mod bindings {
    wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "/tmp/wrpc/hello.sock")]
    path: PathBuf,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::hello::handler::Handler for Server {
    async fn hello(&self) -> std::io::Result<String> {
        Ok("hello from Rust".to_string())
    }
}

async fn serve(
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let mut lis = Listener::new(tx, rx);
    while let Some((Header { instance, name }, srv)) = lis.accept().await? {
        let Ok(fut) = bindings::handle(&Server, &instance, &name, srv) else {
            bail!("`{instance}#{name}` is not exported");
        };
        fut.await?;
    }
    Ok(())
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
                        tasks.spawn(async move {
                            if let Err(err) = serve(tx, rx).await {
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
