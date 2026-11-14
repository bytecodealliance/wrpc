use core::pin::pin;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use tokio::task::JoinSet;
use tokio::{fs, select, signal};
use tracing::{debug, error, info, warn};

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
    /// Path to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "/tmp/wrpc/hello.sock")]
    path: PathBuf,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::hello::handler::Handler<tokio::net::unix::SocketAddr>
    for Server
{
    async fn hello(&self, _: tokio::net::unix::SocketAddr) -> anyhow::Result<String> {
        Ok("hello from Rust".to_string())
    }
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
    let invocations = bindings::serve(wrpc_transport::unix::incoming(lis), Server);
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
