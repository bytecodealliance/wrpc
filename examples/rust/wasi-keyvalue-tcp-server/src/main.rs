use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wasi:keyvalue` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
    let invocations = wrpc_wasi_keyvalue::serve(
        wrpc_transport::tcp::incoming(lis),
        wrpc_wasi_keyvalue_mem::Handler::default(),
    );
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
