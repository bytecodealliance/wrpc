use core::pin::pin;

use anyhow::{Context as _, bail};
use clap::Parser;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};
use wrpc::{Header, Listener};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wasi:keyvalue` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

async fn serve(
    handler: wrpc_wasi_keyvalue_mem::Handler,
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let mut lis = Listener::new(tx, rx);
    while let Some((Header { instance, name }, srv)) = lis.accept().await? {
        let Ok(fut) = wrpc_wasi_keyvalue::handle(&handler, &instance, &name, srv) else {
            bail!("`{instance}#{name}` is not exported");
        };
        fut.await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
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
                        if let Err(err) = stream.set_nodelay(true) {
                            warn!(?err, "failed to set `TCP_NODELAY`");
                        }
                        let (rx, tx) = stream.into_split();
                        let handler = handler.clone();
                        tasks.spawn(async move {
                            if let Err(err) = serve(handler, tx, rx).await {
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
