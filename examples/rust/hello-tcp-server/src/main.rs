use core::net::SocketAddr;
use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
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

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::hello::handler::Handler<SocketAddr> for Server {
    async fn hello(&self, addr: SocketAddr) -> anyhow::Result<String> {
        info!(?addr, "received hello");
        Ok("hello from Rust".to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            res = lis.accept() => {
                match res {
                    Ok((stream, addr)) => {
                        debug!("connection accepted");
                        if let Err(err) = stream.set_nodelay(true) {
                            warn!(?err, "failed to set `TCP_NODELAY`");
                        }
                        let (rx, tx) = stream.into_split();
                        tasks.spawn(async move {
                            if let Err(err) = bindings::serve(addr, tx, rx, Server).await {
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
